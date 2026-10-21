//! Static member-name and property-key occurrence search.
//!
//! Syntax-first locator: it reports where a property name appears in the source, never which
//! property is accessed at runtime. `obj.foo` matches `foo`; `obj["foo"]` and the bounded
//! constant form `obj["f" + "oo"]` also match `foo` as a computed key; `obj[expr]` with a
//! dynamic key does not (that is a lexical reference, reachable through `identifiers`).

use oxc_ast::{
    AstKind,
    ast::{
        ChainElement, Expression, JSXAttributeName, PropertyKey, SimpleAssignmentTarget,
        UnaryExpression,
    },
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{AssignmentOperator, UnaryOperator};
use rxd_core::{Result, Source, perf::Perf};
use serde::Serialize;

use crate::{
    fragment::{Contexts, Target},
    lexical::Locations,
    static_eval,
    strings::JsString,
    structure::Collector,
};

/// Syntactic property access, not object identity or runtime reachability.
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberAccess {
    #[default]
    All,
    Read,
    Write,
}

impl MemberAccess {
    fn matches(self, access: Option<Access>) -> bool {
        match self {
            Self::All => true,
            Self::Read => access.is_some_and(|access| access.read),
            Self::Write => access.is_some_and(|access| access.write),
        }
    }
}

#[derive(Clone, Copy)]
struct Access {
    read: bool,
    write: bool,
}

pub struct Member {
    name: JsString,
    kind: &'static str,
    key_resolution: Option<&'static str>,
    start: u32,
    end: u32,
    entity_id: String,
    context_id: Option<String>,
    access: Option<Access>,
}

impl Member {
    pub fn envelope(&self, _source_id: &str) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            name: &'a JsString,
            member_kind: &'a str,
            key_resolution: Option<&'a str>,
            read: Option<bool>,
            write: Option<bool>,
        }
        crate::envelope::body(
            "member",
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.entity_id),
                context_at: self.context_id.as_deref(),
                start: Some(self.start as usize),
                end: Some(self.end as usize),
                ..Default::default()
            },
            Detail {
                name: &self.name,
                member_kind: self.kind,
                key_resolution: self.key_resolution,
                read: self.access.map(|access| access.read),
                write: self.access.map(|access| access.write),
            },
        )
    }
}

struct Visitor<'a, 's> {
    source: &'s Source,
    locations: Locations<'a, 's>,
    contexts: Contexts,
    parents: Vec<AstKind<'s>>,
    // Only the exact target is written: `obj.p.q = rhs` still reads `obj.p`.
    target: Option<(Span, Access)>,
    region: Span,
    names: crate::names::Names<'a>,
    access: MemberAccess,
    records: Vec<Member>,
    budget: crate::names::RecordBudget,
    error: Option<rxd_core::Error>,
}

impl Visitor<'_, '_> {
    fn member_access(&self, span: Span) -> Access {
        self.target.filter(|(target, _)| *target == span).map_or(
            Access {
                read: true,
                write: false,
            },
            |(_, access)| access,
        )
    }
}

fn static_key_name(
    key: &PropertyKey<'_>,
    computed: bool,
) -> Option<(JsString, Span, Option<&'static str>)> {
    if computed {
        return computed_key(key.as_expression()?)
            .map(|(name, span, resolution)| (name, span, Some(resolution)));
    }
    match key {
        PropertyKey::StaticIdentifier(id) => {
            Some((JsString::from(id.name.as_str()), id.span, None))
        }
        PropertyKey::StringLiteral(literal) => Some((
            JsString::decoded(literal.value.as_str(), literal.lone_surrogates),
            literal.span,
            None,
        )),
        _ => None,
    }
}

/// Resolve a computed access key to a name.
///
/// Only a string literal or a bounded constant expression the evaluator can prove is
/// accepted. A binding lookup would require scope resolution, which this query never does,
/// so `obj[key]` stays out of scope and `obj[0]` is an index rather than a property name.
pub(crate) fn computed_key(expression: &Expression<'_>) -> Option<(JsString, Span, &'static str)> {
    if let Expression::StringLiteral(literal) = expression.get_inner_expression() {
        return Some((
            JsString::decoded(literal.value.as_str(), literal.lone_surrogates),
            literal.span,
            "literal",
        ));
    }
    match static_eval::evaluate(
        expression,
        static_eval::Limits {
            allow_intrinsics: false,
            ..Default::default()
        },
    ) {
        Some(static_eval::Value::String(value)) => Some((value, expression.span(), "static")),
        _ => None,
    }
}

/// Strip only transparent syntax wrappers, never a sequence, call or other value expression.
fn member_span(expression: &Expression<'_>) -> Option<Span> {
    match expression.get_inner_expression() {
        Expression::StaticMemberExpression(member) => Some(member.span),
        Expression::ComputedMemberExpression(member) => Some(member.span),
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::StaticMemberExpression(member) => Some(member.span),
            ChainElement::ComputedMemberExpression(member) => Some(member.span),
            ChainElement::TSNonNullExpression(expression) => member_span(&expression.expression),
            _ => None,
        },
        _ => None,
    }
}

fn target_span(target: &SimpleAssignmentTarget<'_>) -> Option<Span> {
    match target {
        SimpleAssignmentTarget::StaticMemberExpression(member) => Some(member.span),
        SimpleAssignmentTarget::ComputedMemberExpression(member) => Some(member.span),
        SimpleAssignmentTarget::TSAsExpression(expression) => member_span(&expression.expression),
        SimpleAssignmentTarget::TSSatisfiesExpression(expression) => {
            member_span(&expression.expression)
        }
        SimpleAssignmentTarget::TSNonNullExpression(expression) => {
            member_span(&expression.expression)
        }
        SimpleAssignmentTarget::TSTypeAssertion(expression) => member_span(&expression.expression),
        _ => None,
    }
}

impl<'a, 's> Visit<'s> for Visitor<'a, 's> {
    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'s>) {
        // Oxc visits the actual leaves of destructuring and for-in/of targets here;
        // default expressions and computed keys are ordinary expressions, not writes.
        let read = match self.parents.last() {
            Some(AstKind::AssignmentExpression(assignment)) => {
                assignment.operator != AssignmentOperator::Assign
            }
            Some(AstKind::UpdateExpression(_)) => true,
            _ => false,
        };
        let previous = self.target;
        self.target = target_span(target).map(|span| (span, Access { read, write: true }));
        walk::walk_simple_assignment_target(self, target);
        self.target = previous;
    }

    fn visit_unary_expression(&mut self, expression: &UnaryExpression<'s>) {
        let previous = self.target;
        if expression.operator == UnaryOperator::Delete {
            self.target = member_span(&expression.argument).map(|span| {
                (
                    span,
                    Access {
                        read: false,
                        write: true,
                    },
                )
            });
        }
        walk::walk_unary_expression(self, expression);
        self.target = previous;
    }

    fn enter_node(&mut self, node: AstKind<'s>) {
        self.contexts.enter(node, 0);
        self.parents.push(node);
        if self.error.is_some() {
            return;
        }
        let access = match node {
            AstKind::StaticMemberExpression(member) => Some(self.member_access(member.span)),
            AstKind::ComputedMemberExpression(member) => Some(self.member_access(member.span)),
            _ => None,
        };
        let hit = match node {
            AstKind::StaticMemberExpression(member) => Some((
                JsString::from(member.property.name.as_str()),
                member.property.span,
                "static_member",
                None,
            )),
            AstKind::ComputedMemberExpression(member) => computed_key(&member.expression)
                .map(|(name, span, resolution)| (name, span, "computed_key", Some(resolution))),
            AstKind::ObjectProperty(property) => static_key_name(&property.key, property.computed)
                .map(|(name, span, resolution)| {
                    (
                        name,
                        span,
                        if property.method || property.kind != oxc_ast::ast::PropertyKind::Init {
                            "object_method_key"
                        } else {
                            "property_key"
                        },
                        resolution,
                    )
                }),
            AstKind::PropertyDefinition(property) => {
                static_key_name(&property.key, property.computed)
                    .map(|(name, span, resolution)| (name, span, "class_field_key", resolution))
            }
            AstKind::MethodDefinition(method) => static_key_name(&method.key, method.computed)
                .map(|(name, span, resolution)| (name, span, "class_method_key", resolution)),
            AstKind::JSXAttribute(attribute) => match &attribute.name {
                JSXAttributeName::Identifier(id) => Some((
                    JsString::from(id.name.as_str()),
                    id.span,
                    "jsx_attribute_name",
                    None,
                )),
                JSXAttributeName::NamespacedName(_) => None,
            },
            _ => return,
        };
        let Some((name, span, kind, key_resolution)) = hit else {
            return;
        };
        if span.start < self.region.start || span.end > self.region.end {
            return;
        }
        if !self.names.matches(&name) || !self.access.matches(access) {
            return;
        }
        if let Err(error) = self.budget.charge(&name) {
            self.error = Some(error);
            return;
        }
        let location = self.locations.get(span);
        self.records.push(Member {
            name,
            kind,
            key_resolution,
            start: span.start,
            end: span.end,
            entity_id: location.entity_id,
            context_id: self.contexts.current_id(self.source),
            access,
        });
    }

    fn leave_node(&mut self, _node: AstKind<'s>) {
        self.parents.pop();
        self.contexts.leave();
    }
}

pub fn run(
    program: &oxc_ast::ast::Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    names: &crate::NameQuery,
    access: MemberAccess,
    within: Option<&str>,
    perf: &mut Perf,
) -> Result<Vec<Member>> {
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
            parents: Vec::new(),
            target: None,
            region,
            names: names.compile(),
            access,
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
