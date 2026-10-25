//! Syntax-only call/member occurrence search, not runtime resolution.

use oxc_ast::{
    AstKind,
    ast::{ChainElement, Expression},
};
use oxc_ast_visit::Visit;
use oxc_span::Span;
use rxd_core::{Result, Source, perf::Perf};
use serde::Serialize;

use crate::{
    fragment::{Contexts, Target},
    lexical::Locations,
    strings::JsString,
    structure::Collector,
};

pub struct Call {
    callee: Option<JsString>,
    call_kind: &'static str,
    optional: bool,
    argument_count: usize,
    start: u32,
    end: u32,
    entity_id: String,
    context_id: Option<String>,
}

impl Call {
    pub fn envelope(&self) -> impl Serialize {
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
                context_at: self.context_id.as_deref(),
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
    source: &'s Source,
    contexts: Contexts,
    locations: Locations<'a, 's>,
    region: Span,
    names: crate::names::Names<'a>,
    records: Vec<Call>,
    budget: crate::names::RecordBudget,
    error: Option<rxd_core::Error>,
}

fn callee_name(expression: &Expression<'_>) -> Option<(JsString, &'static str)> {
    match expression.get_inner_expression() {
        Expression::Identifier(id) => Some((JsString::from(id.name.as_str()), "direct")),
        Expression::StaticMemberExpression(m) => {
            Some((JsString::from(m.property.name.as_str()), "member"))
        }
        Expression::ComputedMemberExpression(m) => {
            crate::members::computed_key(&m.expression).map(|(name, _, _)| (name, "member"))
        }
        Expression::ChainExpression(c) => match &c.expression {
            ChainElement::StaticMemberExpression(m) => {
                Some((JsString::from(m.property.name.as_str()), "member"))
            }
            ChainElement::ComputedMemberExpression(m) => {
                crate::members::computed_key(&m.expression).map(|(name, _, _)| (name, "member"))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Optional syntax in the callee path, not in arguments or surrounding conditions.
fn optional_path(mut expression: &Expression<'_>) -> bool {
    loop {
        expression = match expression.get_inner_expression() {
            Expression::ChainExpression(_) => return true,
            Expression::StaticMemberExpression(m) => {
                if m.optional {
                    return true;
                }
                &m.object
            }
            Expression::ComputedMemberExpression(m) => {
                if m.optional {
                    return true;
                }
                &m.object
            }
            Expression::CallExpression(c) => {
                if c.optional {
                    return true;
                }
                &c.callee
            }
            _ => return false,
        };
    }
}

impl<'a, 's> Visit<'s> for Visitor<'a, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
        self.contexts.enter(node, 0);
        if self.error.is_some() {
            return;
        }
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
        if !self.names.matches(&name) {
            return;
        }
        if let Err(error) = self.budget.charge(&name) {
            self.error = Some(error);
            return;
        }
        let location = self.locations.get(span);
        self.records.push(Call {
            callee: Some(name),
            call_kind: if kind == "new" { "new" } else { call_kind },
            optional: optional || callee_expr.is_some_and(optional_path),
            argument_count: args,
            start: span.start,
            end: span.end,
            entity_id: location.entity_id,
            context_id: self.contexts.current_id(self.source),
        });
    }

    fn leave_node(&mut self, _node: AstKind<'s>) {
        self.contexts.leave();
    }
}

pub fn run(
    program: &oxc_ast::ast::Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    names: &crate::NameQuery,
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
            source,
            contexts: Contexts::new(Target::Matches),
            locations: Locations::new(collector),
            region,
            names: names.compile(),
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
