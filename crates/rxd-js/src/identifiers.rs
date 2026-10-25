//! Lexical bindings/references only; property names are not identifiers.

use oxc_ast::{AstKind, ast::Program};
use oxc_ast_visit::Visit;
use oxc_span::Span;
use rxd_core::{Result, Source, perf::Perf};
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
    pub fn envelope(&self) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            name: &'a JsString,
            role: &'a str,
            syntax_kind: &'a str,
        }
        crate::envelope::body(
            "identifier",
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.entity_id),
                context_at: self.context_id.as_deref(),
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
    names: crate::names::Names<'a>,
    role: IdentifierRole,
    records: Vec<Identifier>,
    budget: crate::names::RecordBudget,
    error: Option<rxd_core::Error>,
}

impl<'a, 's> Visit<'s> for Visitor<'a, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
        // Mirror the parser traversal's local-context tracking so each hit can
        // report the smallest enclosing syntax fragment.
        self.contexts.enter(node, 0);
        if self.error.is_some() {
            return;
        }
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
        if !self.names.matches(&JsString::from(name)) || !role_matches(self.role, role) {
            return;
        }
        if span.start < self.region.start || span.end > self.region.end {
            return;
        }
        if let Err(error) = self.budget.charge(&JsString::from(name)) {
            self.error = Some(error);
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
    names: &crate::NameQuery,
    role: IdentifierRole,
    within: Option<&str>,
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
            names: names.compile(),
            role,
            records: Vec::new(),
            budget: crate::names::RecordBudget::new(collector.limits),
            error: None,
        };
        visitor.visit_program(program);
        if let Some(error) = visitor.error {
            return Err(error);
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
