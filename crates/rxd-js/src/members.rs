//! Locate static property names and proven constant keys, not dynamic accesses.

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
    decode::Decoders,
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
    pub fn envelope(&self) -> impl Serialize {
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

struct Visitor<'a, 's, 'd> {
    source: &'s Source,
    locations: Locations<'a, 's>,
    contexts: Contexts,
    parents: Vec<AstKind<'s>>,
    // Only the exact target is written: `obj.p.q = rhs` still reads `obj.p`.
    target: Option<(Span, Access)>,
    region: Span,
    names: crate::names::Names<'a>,
    access: MemberAccess,
    decoders: Option<&'d Decoders>,
    records: Vec<Member>,
    budget: crate::names::RecordBudget,
    error: Option<rxd_core::Error>,
}

impl Visitor<'_, '_, '_> {
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
    decoders: Option<&Decoders>,
) -> Option<(JsString, Span, Option<&'static str>)> {
    if computed {
        return computed_key(key.as_expression()?, decoders)
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

/// Accept string literals, proven constant keys and recognized table lookups.
pub(crate) fn computed_key(
    expression: &Expression<'_>,
    decoders: Option<&Decoders>,
) -> Option<(JsString, Span, &'static str)> {
    if let Expression::StringLiteral(literal) = expression.get_inner_expression() {
        return Some((
            JsString::decoded(literal.value.as_str(), literal.lone_surrogates),
            literal.span,
            "literal",
        ));
    }
    if let Some(resolved) = decoders.and_then(|decoders| decoders.expression(expression)) {
        return Some((resolved.value.clone(), expression.span(), "decoder"));
    }
    let result = (static_eval::Context {
        limits: static_eval::Limits {
            allow_intrinsics: false,
            ..Default::default()
        },
        decoders,
    })
    .evaluate(expression)?;
    let origin = result.resolution();
    match result.value {
        static_eval::Value::String(value) => Some((value, expression.span(), origin)),
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

impl<'a, 's, 'd> Visit<'s> for Visitor<'a, 's, 'd> {
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
            AstKind::ComputedMemberExpression(member) => {
                computed_key(&member.expression, self.decoders)
                    .map(|(name, span, resolution)| (name, span, "computed_key", Some(resolution)))
            }
            AstKind::ObjectProperty(property) => {
                static_key_name(&property.key, property.computed, self.decoders).map(
                    |(name, span, resolution)| {
                        (
                            name,
                            span,
                            if property.method || property.kind != oxc_ast::ast::PropertyKind::Init
                            {
                                "object_method_key"
                            } else {
                                "property_key"
                            },
                            resolution,
                        )
                    },
                )
            }
            AstKind::PropertyDefinition(property) => {
                static_key_name(&property.key, property.computed, self.decoders)
                    .map(|(name, span, resolution)| (name, span, "class_field_key", resolution))
            }
            AstKind::MethodDefinition(method) => {
                static_key_name(&method.key, method.computed, self.decoders)
                    .map(|(name, span, resolution)| (name, span, "class_method_key", resolution))
            }
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
            decoders: collector.decoders(),
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
