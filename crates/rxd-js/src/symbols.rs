//! Discover lexical bindings and unresolved reference occurrences in a source region.

use std::collections::HashMap;

use oxc_ast::{AstKind, ast::Program};
use oxc_span::GetSpan;
use rxd_core::{Result, Source, perf::Perf};
use serde::Serialize;

use crate::{
    lexical::{self, Location, Locations},
    strings::JsString,
    structure::Collector,
};

pub struct Symbol {
    id: String,
    name: JsString,
    relation: &'static str,
    location: Location,
    space: Option<&'static str>,
    declaration_ids: Option<Vec<String>>,
}

pub struct UnresolvedReference {
    name: String,
    location: Location,
    type_reference: Option<bool>,
    value_reference: Option<bool>,
}

pub enum Record {
    Symbol(Symbol),
    Unresolved(UnresolvedReference),
}

impl Record {
    fn location(&self) -> &Location {
        match self {
            Self::Symbol(symbol) => &symbol.location,
            Self::Unresolved(reference) => &reference.location,
        }
    }

    pub fn envelope(&self) -> impl Serialize {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum Detail<'a> {
            Symbol {
                name: &'a JsString,
                relation: &'a str,
                space: Option<&'a str>,
                declaration_ids: &'a Option<Vec<String>>,
            },
            Unresolved {
                name: &'a str,
                type_reference: Option<bool>,
                value_reference: Option<bool>,
            },
        }
        let location = self.location();
        let (kind, at, detail) = match self {
            Self::Symbol(symbol) => (
                "symbol",
                Some(symbol.id.as_str()),
                Detail::Symbol {
                    name: &symbol.name,
                    relation: symbol.relation,
                    space: symbol.space,
                    declaration_ids: &symbol.declaration_ids,
                },
            ),
            Self::Unresolved(reference) => (
                "unresolved_reference",
                None,
                Detail::Unresolved {
                    name: &reference.name,
                    type_reference: reference.type_reference,
                    value_reference: reference.value_reference,
                },
            ),
        };
        crate::envelope::body(
            kind,
            rxd_core::envelope::RecordMeta {
                at,
                parent_at: Some(&location.entity_id),
                start: Some(location.start as usize),
                end: Some(location.end as usize),
                ..Default::default()
            },
            detail,
        )
    }
}

pub struct Symbols {
    pub resolution: &'static str,
    pub within_id: String,
    pub records: Vec<Record>,
}

pub fn run(
    program: &Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    names: &crate::NameQuery,
    within: Option<&str>,
    perf: &mut Perf,
) -> Result<Symbols> {
    let root = perf.measure("query", || {
        within
            .map(|id| collector.by_id(id))
            .transpose()
            .map(|i| i.unwrap_or(0))
    })?;
    let region = &collector.entities[root].entity;
    let semantic = lexical::build(program, collector.limits, perf)?;
    perf.measure("query", || {
        if lexical::is_dynamic(&semantic) {
            return Ok(Symbols {
                resolution: "dynamic",
                within_id: region.id.clone(),
                records: Vec::new(),
            });
        }
        let scoping = semantic.scoping();
        // One entry per binding, even when declarations/references repeat.
        let mut bindings = HashMap::new();
        let mut unresolved = Vec::new();
        for node in semantic.nodes().iter() {
            let span = if let AstKind::TSEnumMember(member) = node.kind() {
                member.id.span()
            } else {
                node.span()
            };
            if span.start < region.start || span.end > region.end {
                continue;
            }
            let selected = if let Some((symbol, _)) = lexical::binding(node, &semantic) {
                Some((symbol, true))
            } else {
                match node.kind() {
                    AstKind::IdentifierReference(id) => {
                        let reference = scoping.get_reference(match id.reference_id.get() {
                            Some(id) => id,
                            None => continue,
                        });
                        let symbol = reference.symbol_id();
                        if symbol.is_none() {
                            unresolved.push((
                                span,
                                id.name.as_str(),
                                reference.is_type() || reference.flags().is_value_as_type(),
                                reference.is_value(),
                            ));
                        }
                        symbol.map(|symbol| (symbol, false))
                    }
                    _ => None,
                }
            };
            if let Some((symbol, declared)) = selected {
                if let Some(previous) = bindings.get_mut(&symbol) {
                    *previous |= declared;
                } else {
                    bindings.insert(symbol, declared);
                }
            }
        }
        let names = names.compile();
        let locations = Locations::new(collector);
        let mut records = Vec::with_capacity(bindings.len() + unresolved.len());
        for (symbol, declared) in bindings {
            let name = lexical::name(&semantic, symbol);
            if !names.matches(&name) {
                continue;
            }
            let span = scoping.symbol_span(symbol);
            let declaration_ids = lexical::declaration_ids(&semantic, symbol, source)?;
            records.push(Record::Symbol(Symbol {
                id: source.entity_id("symbol", span.start, span.end),
                name,
                space: program
                    .source_type
                    .is_typescript()
                    .then(|| lexical::space(&semantic, symbol)),
                declaration_ids,
                relation: if declared { "declared" } else { "external" },
                location: locations.get(span),
            }));
        }
        for (span, name, type_reference, value_reference) in unresolved {
            if !names.matches(&JsString::from(name)) {
                continue;
            }
            records.push(Record::Unresolved(UnresolvedReference {
                name: name.to_owned(),
                location: locations.get(span),
                type_reference: program
                    .source_type
                    .is_typescript()
                    .then_some(type_reference),
                value_reference: program
                    .source_type
                    .is_typescript()
                    .then_some(value_reference),
            }));
        }
        records.sort_by_key(|record| {
            let location = record.location();
            (location.start, location.end)
        });
        Ok(Symbols {
            resolution: "static",
            within_id: region.id.clone(),
            records,
        })
    })
}
