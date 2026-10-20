//! Single-file lexical binding queries. Dynamic name resolution is never guessed.

use oxc_ast::{AstKind, ast::Program};
use oxc_span::{GetSpan, Span};
use rxd_core::{Error, Limits, Result, Source, perf::Perf};
use serde::Serialize;

use crate::{
    fragment,
    lexical::{self, Location, Locations},
    strings::JsString,
    structure::Collector,
};

#[derive(Serialize)]
pub struct Selected {
    name: JsString,
    role: &'static str,
    #[serde(flatten)]
    location: Location,
    #[serde(serialize_with = "crate::envelope::serialize_short_option")]
    context_at: Option<String>,
    type_reference: Option<bool>,
    value_reference: Option<bool>,
}

pub struct Reference {
    location: Location,
    /// The binding these references belong to, as the coordinate `get --at` accepts.
    target: String,
    read: bool,
    write: bool,
    member_write: bool,
    context_at: Option<String>,
    type_reference: Option<bool>,
    value_reference: Option<bool>,
}

impl Reference {
    pub fn envelope(&self, _source_id: &str) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            /// The shared skeleton every xref row carries: what this row refers to.
            #[serde(serialize_with = "crate::envelope::serialize_short")]
            target: &'a str,
            read: bool,
            write: bool,
            member_write: bool,
            type_reference: Option<bool>,
            value_reference: Option<bool>,
        }
        crate::envelope::body(
            rxd_core::envelope::REFERENCE,
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.location.entity_id),
                context_at: self.context_at.as_deref(),
                start: Some(self.location.start as usize),
                end: Some(self.location.end as usize),
                ..Default::default()
            },
            Detail {
                target: &self.target,
                read: self.read,
                write: self.write,
                member_write: self.member_write,
                type_reference: self.type_reference,
                value_reference: self.value_reference,
            },
        )
    }
}

pub struct References {
    pub resolution: &'static str,
    pub selected: Selected,
    pub declaration: Option<Location>,
    pub references: Vec<Reference>,
    pub declaration_ids: Option<Vec<String>>,
}

pub fn run(
    program: &Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    byte: u64,
    limits: Limits,
    perf: &mut Perf,
) -> Result<References> {
    if byte >= source.text.len() as u64 {
        return Err(Error::new(
            "INVALID_QUERY",
            "byte offset must be smaller than file length",
        ));
    }
    let semantic = lexical::build(program, limits, perf)?;
    perf.measure("query", || {
        let scoping = semantic.scoping();
        let typed = program.source_type.is_typescript();
        let context_id = |node_id| {
            std::iter::once(semantic.nodes().kind(node_id))
                .chain(semantic.nodes().ancestor_kinds(node_id))
                .take_while(|&node| !fragment::boundary(node))
                .find_map(fragment::key)
                .map(|key| key.id(source))
        };
        let selected = semantic
            .nodes()
            .iter_enumerated()
            .find_map(|(node_id, node)| {
                let span = if let AstKind::TSEnumMember(member) = node.kind() {
                    member.id.span()
                } else {
                    node.span()
                };
                if !(u64::from(span.start) <= byte && byte < u64::from(span.end)) {
                    return None;
                }
                if let Some((symbol, span)) = lexical::binding(node, &semantic) {
                    return Some((
                        lexical::name(&semantic, symbol),
                        "declaration",
                        span,
                        Some(symbol),
                        node_id,
                        None,
                    ));
                }
                if let AstKind::IdentifierReference(id) = node.kind() {
                    let reference = scoping.get_reference(id.reference_id.get()?);
                    return Some((
                        JsString::from(id.name.as_str()),
                        rxd_core::envelope::REFERENCE,
                        span,
                        reference.symbol_id(),
                        node_id,
                        Some((
                            reference.is_type() || reference.flags().is_value_as_type(),
                            reference.is_value(),
                        )),
                    ));
                }
                None
            })
            .ok_or_else(|| {
                Error::new(
                    "NOT_FOUND",
                    "byte offset must select a binding identifier or identifier reference",
                )
            })?;
        let (name, role, span, symbol, node_id, reference_kind) = selected;
        let locations = Locations::new(collector);
        let location = |span: Span| locations.get(span);
        let selected = Selected {
            name,
            role,
            location: location(span),
            context_at: context_id(node_id),
            type_reference: if typed {
                reference_kind.map(|kind| kind.0)
            } else {
                None
            },
            value_reference: if typed {
                reference_kind.map(|kind| kind.1)
            } else {
                None
            },
        };
        // File-wide conservatism: with/eval can invalidate lexical reference claims.
        let dynamic = lexical::is_dynamic(&semantic);
        if dynamic || symbol.is_none() {
            return Ok(References {
                resolution: if dynamic { "dynamic" } else { "unresolved" },
                selected,
                declaration: None,
                references: Vec::new(),
                declaration_ids: None,
            });
        }
        let symbol = symbol.expect("resolved symbol");
        if scoping.get_resolved_reference_ids(symbol).len() > limits.records {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "output record budget exceeded",
            ));
        }
        let declaration_location = location(scoping.symbol_span(symbol));
        let target = declaration_location.entity_id.clone();
        let declaration = Some(declaration_location);
        let mut references: Vec<_> = semantic
            .symbol_references(symbol)
            .map(|reference| Reference {
                location: location(semantic.reference_span(reference)),
                target: target.clone(),
                read: reference.is_read(),
                write: reference.is_write(),
                member_write: reference.flags().is_member_write_target(),
                context_at: context_id(reference.node_id()),
                type_reference: typed.then(|| reference.is_type()),
                value_reference: typed.then(|| reference.is_value()),
            })
            .collect();
        references.sort_by_key(|reference| (reference.location.start, reference.location.end));
        Ok(References {
            resolution: "resolved",
            selected,
            declaration,
            references,
            declaration_ids: lexical::declaration_ids(
                &semantic,
                symbol,
                source,
                Limits {
                    records: limits.records - scoping.get_resolved_reference_ids(symbol).len(),
                    ..limits
                },
            )?,
        })
    })
}
