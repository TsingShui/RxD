//! Static member-name and property-key occurrence search.
//!
//! Syntax-first locator: it reports where a property name appears in the source, never which
//! property is accessed at runtime. `obj.foo` matches `foo`; `obj["foo"]` and the bounded
//! constant form `obj["f" + "oo"]` also match `foo` as a computed key; `obj[expr]` with a
//! dynamic key does not (that is a lexical reference, reachable through `identifiers`).

use oxc_ast::{
    AstKind,
    ast::{Expression, JSXAttributeName, PropertyKey},
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};
use rxd_core::{Result, Source, perf::Perf};
use serde::Serialize;

use crate::{lexical::Locations, static_eval, strings::JsString, structure::Collector};

pub struct Member {
    name: JsString,
    kind: &'static str,
    key_resolution: Option<&'static str>,
    start: u32,
    end: u32,
    entity_id: String,
}

impl Member {
    pub fn envelope(&self, _source_id: &str) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            name: &'a JsString,
            member_kind: &'a str,
            key_resolution: Option<&'a str>,
        }
        crate::envelope::body(
            "member",
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.entity_id),
                start: Some(self.start as usize),
                end: Some(self.end as usize),
                ..Default::default()
            },
            Detail {
                name: &self.name,
                member_kind: self.kind,
                key_resolution: self.key_resolution,
            },
        )
    }
}

struct Visitor<'a, 's> {
    locations: Locations<'a, 's>,
    region: Span,
    wanted: &'a str,
    records: Vec<Member>,
}

fn static_key_name<'a>(key: &'a PropertyKey<'a>, computed: bool) -> Option<(&'a str, Span)> {
    if computed {
        return None;
    }
    match key {
        PropertyKey::StaticIdentifier(id) => Some((id.name.as_str(), id.span)),
        PropertyKey::StringLiteral(literal) => Some((literal.value.as_str(), literal.span)),
        _ => None,
    }
}

/// Resolve a computed access key to a name.
///
/// Only a string literal or a bounded constant expression the evaluator can prove is
/// accepted. A binding lookup would require scope resolution, which this query never does,
/// so `obj[key]` stays out of scope and `obj[0]` is an index rather than a property name.
fn computed_key(expression: &Expression<'_>) -> Option<(JsString, Span, &'static str)> {
    if let Expression::StringLiteral(literal) = expression.without_parentheses() {
        return Some((
            JsString::decoded(literal.value.as_str(), literal.lone_surrogates),
            literal.span,
            "literal",
        ));
    }
    match static_eval::evaluate(expression, static_eval::Limits::default()) {
        Some(static_eval::Value::String(value)) => {
            Some((JsString::from(value.as_str()), expression.span(), "static"))
        }
        _ => None,
    }
}

impl<'a, 's> Visit<'s> for Visitor<'a, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
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
                .map(|(name, span)| {
                    (
                        JsString::from(name),
                        span,
                        if property.method || property.kind != oxc_ast::ast::PropertyKind::Init {
                            "object_method_key"
                        } else {
                            "property_key"
                        },
                        None,
                    )
                }),
            AstKind::PropertyDefinition(property) => {
                static_key_name(&property.key, property.computed)
                    .map(|(name, span)| (JsString::from(name), span, "class_field_key", None))
            }
            AstKind::MethodDefinition(method) => static_key_name(&method.key, method.computed)
                .map(|(name, span)| (JsString::from(name), span, "class_method_key", None)),
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
        if !name.equals_str(self.wanted) {
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
