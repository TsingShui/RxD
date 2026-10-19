//! Exact lexical identifier occurrence search.
//!
//! This is a syntax-first query. It returns binding identifiers and identifier references
//! without treating static member/property names as lexical identifiers.

use oxc_ast::{AstKind, ast::Program};
use oxc_ast_visit::Visit;
use oxc_span::Span;
use rxd_core::{Error, Limits, Result, Source, perf::Perf};
use serde::Serialize;

use crate::{
    IdentifierRole,
    fragment::{Contexts, Target},
    lexical::Locations,
    strings::JsString,
    structure::Collector,
};

pub struct Identifier {
    pub name: JsString,
    pub role: &'static str,
    pub kind: &'static str,
    pub start: u32,
    pub end: u32,
    pub entity_id: String,
    pub context_id: Option<String>,
}

impl Identifier {
    pub fn envelope(&self, source_id: &str) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            name: &'a JsString,
            role: &'a str,
            syntax_kind: &'a str,
        }
        crate::envelope::body(
            "identifier",
            rxd_core::envelope::RecordMeta {
                source_id: Some(source_id),
                parent_id: Some(&self.entity_id),
                context_id: self.context_id.as_deref(),
                start: Some(self.start as usize),
                end: Some(self.end as usize),
                ..Default::default()
            },
            Detail {
                name: &self.name,
                role: self.role,
                syntax_kind: self.kind,
            },
        )
    }
}

struct Visitor<'a, 's> {
    source: &'s Source,
    locations: Locations<'a, 's>,
    contexts: Contexts,
    region: Span,
    wanted: &'a str,
    role: IdentifierRole,
    records: Vec<Identifier>,
}

impl<'a, 's> Visit<'s> for Visitor<'a, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
        // Mirror the parser traversal's local-context tracking so each hit can
        // report the smallest enclosing syntax fragment.
        self.contexts.enter(node, 0);
        let (name, role, kind, span) = match node {
            AstKind::BindingIdentifier(identifier) => (
                identifier.name.as_str(),
                IdentifierRole::Declaration,
                "binding_identifier",
                identifier.span,
            ),
            AstKind::IdentifierReference(identifier) => (
                identifier.name.as_str(),
                IdentifierRole::Reference,
                "identifier_reference",
                identifier.span,
            ),
            _ => return,
        };
        if name != self.wanted || !role_matches(self.role, role) {
            return;
        }
        if span.start < self.region.start || span.end > self.region.end {
            return;
        }
        let location = self.locations.get(span);
        let context_id = self.contexts.current_id(self.source);
        self.records.push(Identifier {
            name: JsString::from(name),
            role: match role {
                IdentifierRole::Declaration => "declaration",
                IdentifierRole::Reference => "reference",
                IdentifierRole::All => unreachable!(),
            },
            kind,
            start: span.start,
            end: span.end,
            entity_id: location.entity_id,
            context_id,
        });
    }

    fn leave_node(&mut self, _node: AstKind<'s>) {
        self.contexts.leave();
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    program: &Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    wanted: &str,
    role: IdentifierRole,
    within: Option<&str>,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<Identifier>> {
    let region = match within {
        Some(id) => {
            let index = collector.by_id(id)?;
            let entity = &collector.entities[index].entity;
            Span::new(entity.start, entity.end)
        }
        None => Span::new(0, source.text.len() as u32),
    };
    perf.measure("query", || {
        let mut visitor = Visitor {
            source,
            locations: Locations::new(collector),
            contexts: Contexts::new(Target::Matches),
            region,
            wanted,
            role,
            records: Vec::new(),
        };
        visitor.visit_program(program);
        if visitor.records.len() > limits.records {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "identifier record budget exceeded",
            ));
        }
        visitor
            .records
            .sort_by_key(|record| (record.start, record.end));
        Ok(visitor.records)
    })
}

fn role_matches(wanted: IdentifierRole, actual: IdentifierRole) -> bool {
    matches!(wanted, IdentifierRole::All)
        || std::mem::discriminant(&wanted) == std::mem::discriminant(&actual)
}
