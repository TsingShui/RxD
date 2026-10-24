//! Single-file lexical binding queries. Dynamic name resolution is never guessed.

use oxc_ast::{AstKind, ast::Program};
use oxc_semantic::{NodeId, Semantic, SymbolId};
use oxc_span::{GetSpan, Span};
use rxd_core::{Error, Limits, Result, Source, filter::Contains, perf::Perf};
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
    pub fn envelope(&self) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            /// The shared skeleton every xref row carries: what this row refers to.
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
    pub references: Vec<Reference>,
}

/// The reference rows of one resolved symbol, and where its declaration is.
fn symbol_references(
    semantic: &Semantic<'_>,
    symbol: SymbolId,
    source: &Source,
    locations: &Locations<'_, '_>,
    typed: bool,
) -> Result<(Location, Vec<Reference>)> {
    let scoping = semantic.scoping();
    let declaration = locations.get(scoping.symbol_span(symbol));
    let target = declaration.entity_id.clone();
    let mut references: Vec<_> = semantic
        .symbol_references(symbol)
        .map(|reference| Reference {
            location: locations.get(semantic.reference_span(reference)),
            target: target.clone(),
            read: reference.is_read(),
            write: reference.is_write(),
            member_write: reference.flags().is_member_write_target(),
            context_at: context_at(semantic, reference.node_id(), source),
            type_reference: typed.then(|| reference.is_type()),
            value_reference: typed.then(|| reference.is_value()),
        })
        .collect();
    references.sort_by_key(|reference| (reference.location.start, reference.location.end));
    Ok((declaration, references))
}

/// Every reference to a binding whose name matches a filter.
///
/// `-f PATTERN --kind symbol` is the content-search face of `xref`: one row per
/// (target, site), each row naming its target, in position order across every match.
pub fn run_filter(
    program: &Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    pattern: &Contains,
    case_sensitive: bool,
    limits: Limits,
    perf: &mut Perf,
) -> Result<FilterReferences> {
    let semantic = lexical::build(program, limits, perf)?;
    perf.measure("query", || {
        let typed = program.source_type.is_typescript();
        let locations = Locations::new(collector);
        if lexical::is_dynamic(&semantic) {
            return Ok(FilterReferences {
                resolution: "dynamic",
                targets: 0,
                references: Vec::new(),
            });
        }
        let mut targets = 0usize;
        let mut references = Vec::new();
        for symbol in semantic.scoping().symbol_ids() {
            if !lexical::name(&semantic, symbol).contains_filter(pattern.text(), case_sensitive) {
                continue;
            }
            targets += 1;
            let (_, mut rows) = symbol_references(&semantic, symbol, source, &locations, typed)?;
            references.append(&mut rows);
        }
        references.sort_by_key(|reference| {
            (
                reference.location.start,
                reference.location.end,
                reference.target.clone(),
            )
        });
        Ok(FilterReferences {
            resolution: "resolved",
            targets,
            references,
        })
    })
}

/// The result of a name-filtered xref: no single selected occurrence.
pub struct FilterReferences {
    pub resolution: &'static str,
    pub targets: usize,
    pub references: Vec<Reference>,
}

/// The fragment id path of the node that owns one occurrence, for a row's `context_at`.
fn context_at(semantic: &Semantic<'_>, node_id: NodeId, source: &Source) -> Option<String> {
    std::iter::once(semantic.nodes().kind(node_id))
        .chain(semantic.nodes().ancestor_kinds(node_id))
        .take_while(|&node| !fragment::boundary(node))
        .find_map(fragment::key)
        .map(|key| key.id(source))
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
        let ctx = |node_id| context_at(&semantic, node_id, source);
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
            context_at: ctx(node_id),
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
            let _ = (dynamic, selected);
            return Ok(References {
                references: Vec::new(),
            });
        }
        let symbol = symbol.expect("resolved symbol");
        let (_, references) = symbol_references(&semantic, symbol, source, &locations, typed)?;
        let _ = selected;
        Ok(References { references })
    })
}
