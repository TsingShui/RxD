//! Source-backed value slots, not an object evaluator or JSON conversion.
use crate::{
    fragment::{Contexts, Target},
    lexical::Locations,
    strings::JsString,
    structure::Collector,
};
use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};
use rxd_core::{Error, Limits, Result, Source, perf::Perf};
use serde::Serialize;

pub const VALUE_KINDS: &[&str] = &[
    "initializer",
    "property",
    "element",
    "argument",
    "assignment",
];
#[derive(Clone, Default)]
pub struct ValuesQuery {
    pub within: Option<String>,
    pub kind: Option<String>,
    pub names: crate::NameQuery,
}
impl ValuesQuery {
    pub fn validate(&self) -> Result<()> {
        if self
            .kind
            .as_deref()
            .is_some_and(|s| !VALUE_KINDS.contains(&s))
        {
            return Err(Error::new("INVALID_QUERY", "invalid JS value kind"));
        }
        self.names.validate()
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Primitive {
    String(JsString),
    Number(f64),
    Boolean(bool),
}
#[derive(Serialize)]
struct ExpressionValue<'a> {
    start: u32,
    end: u32,
    kind: &'static str,
    resolution: &'static str,
    value: Option<Primitive>,
    source: Option<&'a str>,
    unresolved_reason: Option<&'static str>,
}
fn expression<'s>(
    source: &'s Source,
    e: Option<&Expression<'_>>,
    span: Span,
    forced: Option<&'static str>,
) -> ExpressionValue<'s> {
    let mut out = ExpressionValue {
        start: span.start,
        end: span.end,
        kind: "expression",
        resolution: "unresolved",
        value: None,
        source: Some(&source.text[span.start as usize..span.end as usize]),
        unresolved_reason: forced,
    };
    let Some(e) = e else {
        out.kind = "hole";
        out.resolution = "hole";
        out.source = None;
        return out;
    };
    let inner = e.get_inner_expression();
    out.kind = match inner {
        Expression::ObjectExpression(_) => "object",
        Expression::ArrayExpression(_) => "array",
        Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => "string",
        Expression::NumericLiteral(_) => "number",
        Expression::BooleanLiteral(_) => "boolean",
        Expression::NullLiteral(_) => "null",
        Expression::Identifier(_) => "identifier",
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_) => "function",
        _ => "expression",
    };
    if forced.is_some() {
        return out;
    }
    match inner {
        Expression::ObjectExpression(_) | Expression::ArrayExpression(_) => {
            out.resolution = "container"
        }
        Expression::NumericLiteral(n) if n.value.is_finite() => {
            out.resolution = "literal";
            out.value = Some(Primitive::Number(n.value));
        }
        Expression::BooleanLiteral(b) => {
            out.resolution = "literal";
            out.value = Some(Primitive::Boolean(b.value));
        }
        Expression::NullLiteral(_) => out.resolution = "literal",
        _ => {
            let v = crate::resources::value(source, e, span, None);
            if v.resolution != "unresolved" {
                out.kind = "string";
                out.resolution = v.resolution;
                out.value = Some(Primitive::String(v.name));
            } else {
                out.unresolved_reason = v.unresolved_reason;
            }
        }
    }
    if out.resolution != "unresolved" {
        out.source = None;
    }
    out
}

#[derive(Serialize)]
struct Detail<'s> {
    kind: &'static str,
    name: JsString,
    syntax_kind: &'static str,
    owner_start: u32,
    owner_end: u32,
    index: Option<usize>,
    computed: bool,
    spread: bool,
    key_start: Option<u32>,
    key_end: Option<u32>,
    key_resolution: Option<&'static str>,
    value: ExpressionValue<'s>,
}
pub struct Value<'s> {
    span: Span,
    parent: String,
    context: Option<String>,
    detail: Detail<'s>,
}
impl Value<'_> {
    pub fn envelope(&self) -> impl Serialize {
        crate::envelope::body(
            "value",
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.parent),
                context_at: self.context.as_deref(),
                start: Some(self.span.start as usize),
                end: Some(self.span.end as usize),
                ..Default::default()
            },
            &self.detail,
        )
    }
}
struct Visitor<'c, 's> {
    source: &'s Source,
    query: &'c ValuesQuery,
    names: crate::names::Names<'c>,
    locations: Locations<'c, 's>,
    contexts: Contexts,
    region: Span,
    records: Vec<Value<'s>>,
    bytes: usize,
    limits: Limits,
    error: Option<Error>,
}
impl<'s> Visitor<'_, 's> {
    fn add(&mut self, site: Span, detail: Detail<'s>) {
        if self.error.is_some()
            || site.start < self.region.start
            || site.end > self.region.end
            || self.query.kind.as_deref().is_some_and(|k| k != detail.kind)
            || !self.names.matches(&detail.name)
        {
            return;
        }
        let budget = self.limits;
        // Bound owned labels and retained source projections before retaining a row.
        let bytes = detail.name.len_utf16() * 2
            + detail.value.source.map_or(0, str::len)
            + match &detail.value.value {
                Some(Primitive::String(s)) => s.len_utf16() * 2,
                _ => 0,
            }
            + 512;
        self.bytes = self.bytes.saturating_add(bytes);
        if self.records.len() >= budget.work || self.bytes > budget.metadata_bytes {
            self.error = Some(Error::new(
                "RESOURCE_LIMIT",
                "JS value projection budget exceeded",
            ));
            return;
        }
        self.records.push(Value {
            span: site,
            parent: self.locations.get(site).entity_id,
            context: self.contexts.current_id(self.source),
            detail,
        });
    }
    fn detail(
        &self,
        kind: &'static str,
        name: JsString,
        syntax: &'static str,
        owner: Span,
        value: ExpressionValue<'s>,
    ) -> Detail<'s> {
        Detail {
            kind,
            name,
            syntax_kind: syntax,
            owner_start: owner.start,
            owner_end: owner.end,
            index: None,
            computed: false,
            spread: false,
            key_start: None,
            key_end: None,
            key_resolution: None,
            value,
        }
    }
    fn text(&self, span: Span) -> JsString {
        JsString::from(&self.source.text[span.start as usize..span.end as usize])
    }
    fn arguments(&mut self, args: &[Argument<'_>], owner: Span, syntax: &'static str) {
        for (i, arg) in args.iter().enumerate() {
            let (e, spread) = match arg {
                Argument::SpreadElement(s) => (&s.argument, true),
                a => {
                    let Some(e) = a.as_expression() else { continue };
                    (e, false)
                }
            };
            let mut d = self.detail(
                "argument",
                JsString::from(i.to_string().as_str()),
                syntax,
                owner,
                expression(self.source, Some(e), arg.span(), spread.then_some("spread")),
            );
            d.index = Some(i);
            d.spread = spread;
            self.add(arg.span(), d);
        }
    }
}
impl<'s> Visit<'s> for Visitor<'_, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
        self.contexts.enter(node, 0);
        if self.error.is_some() {
            return;
        }
        let span = node.span();
        if span.end < self.region.start || span.start > self.region.end {
            return;
        }
        match node {
            AstKind::VariableDeclarator(d) => {
                if let Some(e) = &d.init {
                    self.add(
                        d.span,
                        self.detail(
                            "initializer",
                            self.text(d.id.span()),
                            "variable_declarator",
                            d.span,
                            expression(self.source, Some(e), e.span(), None),
                        ),
                    );
                }
            }
            AstKind::ObjectExpression(o) => {
                for (i, p) in o.properties.iter().enumerate() {
                    match p {
                        ObjectPropertyKind::ObjectProperty(p) => {
                            let key = p.key.span();
                            let (name, resolution) = if p.computed {
                                let v = p
                                    .key
                                    .as_expression()
                                    .map(|e| crate::resources::value(self.source, e, key, None));
                                match v {
                                    Some(v) if v.resolution != "unresolved" => {
                                        (v.name, v.resolution)
                                    }
                                    _ => (self.text(key), "unresolved"),
                                }
                            } else {
                                match &p.key {
                                    PropertyKey::StringLiteral(s) => (
                                        JsString::decoded(s.value.as_str(), s.lone_surrogates),
                                        "literal",
                                    ),
                                    _ => p.key.static_name().map_or_else(
                                        || (self.text(key), "unresolved"),
                                        |n| (JsString::from(n.as_ref()), "literal"),
                                    ),
                                }
                            };
                            let syntax = match p.kind {
                                PropertyKind::Get => "get",
                                PropertyKind::Set => "set",
                                _ if p.method => "method",
                                _ if p.shorthand => "shorthand",
                                _ => "property",
                            };
                            let forced = (p.method || p.kind != PropertyKind::Init)
                                .then_some("method_or_accessor");
                            let mut d = self.detail(
                                "property",
                                name,
                                syntax,
                                o.span,
                                expression(self.source, Some(&p.value), p.value.span(), forced),
                            );
                            d.index = Some(i);
                            d.computed = p.computed;
                            d.key_start = Some(key.start);
                            d.key_end = Some(key.end);
                            d.key_resolution = Some(resolution);
                            // ObjectProperty is visited later; use its real sliceable fragment now.
                            self.contexts.enter(AstKind::ObjectProperty(p), 0);
                            self.add(p.span, d);
                            self.contexts.leave();
                        }
                        ObjectPropertyKind::SpreadProperty(p) => {
                            let mut d = self.detail(
                                "property",
                                self.text(p.span),
                                "spread",
                                o.span,
                                expression(self.source, Some(&p.argument), p.span, Some("spread")),
                            );
                            d.index = Some(i);
                            d.spread = true;
                            self.add(p.span, d);
                        }
                    }
                }
            }
            AstKind::ArrayExpression(a) => {
                for (i, e) in a.elements.iter().enumerate() {
                    let (expr, spread, syntax) = match e {
                        ArrayExpressionElement::Elision(_) => (None, false, "hole"),
                        ArrayExpressionElement::SpreadElement(s) => {
                            (Some(&s.argument), true, "spread")
                        }
                        e => (e.as_expression(), false, "element"),
                    };
                    let mut d = self.detail(
                        "element",
                        JsString::from(i.to_string().as_str()),
                        syntax,
                        a.span,
                        expression(self.source, expr, e.span(), spread.then_some("spread")),
                    );
                    d.index = Some(i);
                    d.spread = spread;
                    self.add(e.span(), d);
                }
            }
            AstKind::CallExpression(c) => self.arguments(&c.arguments, c.span, "call"),
            AstKind::NewExpression(c) => self.arguments(&c.arguments, c.span, "new"),
            AstKind::ImportExpression(c) => {
                let mut d = self.detail(
                    "argument",
                    JsString::from("0"),
                    "import",
                    c.span,
                    expression(self.source, Some(&c.source), c.source.span(), None),
                );
                d.index = Some(0);
                self.add(c.source.span(), d);
                if let Some(options) = &c.options {
                    let mut d = self.detail(
                        "argument",
                        JsString::from("1"),
                        "import",
                        c.span,
                        expression(self.source, Some(options), options.span(), None),
                    );
                    d.index = Some(1);
                    self.add(options.span(), d);
                }
            }
            AstKind::AssignmentExpression(a) => {
                let forced = (a.operator != oxc_syntax::operator::AssignmentOperator::Assign)
                    .then_some("compound_assignment");
                self.add(
                    a.span,
                    self.detail(
                        "assignment",
                        self.text(a.left.span()),
                        a.operator.as_str(),
                        a.span,
                        expression(self.source, Some(&a.right), a.right.span(), forced),
                    ),
                );
            }
            _ => {}
        }
    }
    fn leave_node(&mut self, _: AstKind<'s>) {
        self.contexts.leave();
    }
}
pub fn run<'s>(
    program: &Program<'s>,
    source: &'s Source,
    collector: &Collector<'s>,
    query: &ValuesQuery,
    region: Span,
    perf: &mut Perf,
) -> Result<Vec<Value<'s>>> {
    perf.measure("query", || {
        let mut v = Visitor {
            source,
            query,
            names: query.names.compile(),
            locations: Locations::new(collector),
            contexts: Contexts::new(Target::Matches),
            region,
            records: Vec::new(),
            bytes: 0,
            limits: collector.limits,
            error: None,
        };
        v.visit_program(program);
        if let Some(e) = v.error {
            return Err(e);
        }
        v.records
            .sort_by_key(|r| (r.span.start, r.span.end, r.detail.kind));
        Ok(v.records)
    })
}
