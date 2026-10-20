//! Call-site and static member-name occurrence search.
//!
//! Like `identifiers`, this is a syntax-first locator. It reports where a callee or
//! property name appears, never what a call returns or which property is accessed at runtime.

use oxc_ast::{AstKind, ast::Expression};
use oxc_ast_visit::Visit;
use oxc_span::Span;
use rxd_core::{Result, Source, perf::Perf};
use serde::Serialize;

use crate::{lexical::Locations, strings::JsString, structure::Collector};

pub struct Call {
    callee: Option<JsString>,
    call_kind: &'static str,
    optional: bool,
    argument_count: usize,
    start: u32,
    end: u32,
    entity_id: String,
}

impl Call {
    pub fn envelope(&self, _source_id: &str) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            callee: &'a Option<JsString>,
            call_kind: &'a str,
            optional: bool,
            argument_count: usize,
        }
        crate::envelope::body(
            "call",
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.entity_id),
                start: Some(self.start as usize),
                end: Some(self.end as usize),
                ..Default::default()
            },
            Detail {
                callee: &self.callee,
                call_kind: self.call_kind,
                optional: self.optional,
                argument_count: self.argument_count,
            },
        )
    }
}

struct Visitor<'a, 's> {
    locations: Locations<'a, 's>,
    region: Span,
    wanted: &'a str,
    records: Vec<Call>,
}

fn callee_name(expression: &Expression<'_>) -> Option<(JsString, &'static str)> {
    match expression.without_parentheses() {
        Expression::Identifier(identifier) => {
            Some((JsString::from(identifier.name.as_str()), "direct"))
        }
        Expression::StaticMemberExpression(member) => {
            Some((JsString::from(member.property.name.as_str()), "member"))
        }
        _ => None,
    }
}

impl<'a, 's> Visit<'s> for Visitor<'a, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
        let (span, callee_expr, optional, args, kind) = match node {
            AstKind::CallExpression(call) => (
                call.span,
                Some(&call.callee),
                call.optional,
                call.arguments.len(),
                "call",
            ),
            AstKind::NewExpression(new) => (
                new.span,
                Some(&new.callee),
                false,
                new.arguments.len(),
                "new",
            ),
            _ => return,
        };
        if span.start < self.region.start || span.end > self.region.end {
            return;
        }
        let named = callee_expr.and_then(|expr| callee_name(expr));
        let Some((name, call_kind)) = named else {
            return;
        };
        if !name.equals_str(self.wanted) {
            return;
        }
        let location = self.locations.get(span);
        self.records.push(Call {
            callee: Some(name),
            call_kind: if kind == "new" { "new" } else { call_kind },
            optional,
            argument_count: args,
            start: span.start,
            end: span.end,
            entity_id: location.entity_id,
        });
    }
}

pub fn run(
    program: &oxc_ast::ast::Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    wanted: &str,
    within: Option<&str>,
    perf: &mut Perf,
) -> Result<Vec<Call>> {
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
            locations: Locations::new(collector),
            region,
            wanted,
            records: Vec::new(),
        };
        visitor.visit_program(program);
        visitor
            .records
            .sort_by_key(|record| (record.start, record.end));
        Ok(visitor.records)
    })
}
