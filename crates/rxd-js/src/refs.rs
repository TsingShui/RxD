//! Single-file lexical binding queries. Dynamic name resolution is never guessed.

use oxc_ast::{AstKind, ast::Program};
use oxc_semantic::{NodeId, Semantic, SymbolId};
use oxc_span::GetSpan;
use rxd_core::{Error, Limits, Result, Source, filter::Contains, perf::Perf};
use serde::Serialize;

use crate::{
    fragment,
    lexical::{self, Location, Locations},
    structure::Collector,
};

pub struct Reference {
    location: Location,
    /// The binding these references belong to, as the coordinate `slice --at` accepts.
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

/// The reference rows of one resolved symbol.
fn symbol_references(
    semantic: &Semantic<'_>,
    symbol: SymbolId,
    source: &Source,
    locations: &Locations<'_, '_>,
    typed: bool,
) -> Vec<Reference> {
    let scoping = semantic.scoping();
    let target = locations.get(scoping.symbol_span(symbol)).entity_id;
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
    references
}

/// Every reference to a binding whose name matches a filter.
///
/// `-m PATTERN --kind symbol` is the content-search face of `xref`: one row per
/// (target, site), each row naming its target, in position order across every match.
pub fn run_filter(
    program: &Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    pattern: &Contains,
    case_sensitive: bool,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<Reference>> {
    let semantic = lexical::build(program, limits, perf)?;
    perf.measure("query", || {
        let typed = program.source_type.is_typescript();
        if lexical::is_dynamic(&semantic) {
            return Ok(Vec::new());
        }
        let locations = Locations::new(collector);
        let mut references = Vec::new();
        for symbol in semantic.scoping().symbol_ids() {
            if !lexical::name(&semantic, symbol).contains_filter(pattern.text(), case_sensitive) {
                continue;
            }
            let mut rows = symbol_references(&semantic, symbol, source, &locations, typed);
            references.append(&mut rows);
        }
        references.sort_by_key(|reference| {
            (
                reference.location.start,
                reference.location.end,
                reference.target.clone(),
            )
        });
        Ok(references)
    })
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
) -> Result<Vec<Reference>> {
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
        // Validate that the selected byte belongs to a binding or reference before
        // applying file-wide dynamic-scope conservatism. An unresolved reference is
        // still a valid occurrence, so retain the nested option.
        let symbol = semantic
            .nodes()
            .iter()
            .find_map(|node| {
                let span = if let AstKind::TSEnumMember(member) = node.kind() {
                    member.id.span()
                } else {
                    node.span()
                };
                if !(u64::from(span.start) <= byte && byte < u64::from(span.end)) {
                    return None;
                }
                if let Some((symbol, _)) = lexical::binding(node, &semantic) {
                    return Some(Some(symbol));
                }
                if let AstKind::IdentifierReference(id) = node.kind() {
                    return Some(scoping.get_reference(id.reference_id.get()?).symbol_id());
                }
                None
            })
            .ok_or_else(|| {
                Error::new(
                    "NOT_FOUND",
                    "byte offset must select a binding identifier or identifier reference",
                )
            })?;
        // File-wide conservatism: with/eval can invalidate lexical reference claims.
        if lexical::is_dynamic(&semantic) || symbol.is_none() {
            return Ok(Vec::new());
        }
        let symbol = symbol.expect("resolved symbol");
        let locations = Locations::new(collector);
        Ok(symbol_references(
            &semantic, symbol, source, &locations, typed,
        ))
    })
}
