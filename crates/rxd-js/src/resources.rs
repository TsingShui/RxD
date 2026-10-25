//! Syntax-backed loading candidates. No URL fetching, binding resolution or input execution.

use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator};
use rxd_core::{Error, Result, Source, perf::Perf};
use serde::Serialize;

use crate::{
    bindings::Bindings,
    fragment::{Contexts, Target},
    lexical::Locations,
    strings::JsString,
    structure::Collector,
};

pub const RESOURCE_KINDS: &[&str] = &["module", "script", "worker", "request", "url", "attribute"];
const FOLD_DEPTH: usize = 32;
const FOLD_STEPS: usize = 512;
const FOLD_UNITS: usize = 32 * 1024;

#[derive(Clone, Default)]
pub struct ResourcesQuery {
    pub kind: Option<String>,
    /// `resolved` means a known string, not a resolved URL or runtime resource identity.
    pub resolution: Option<String>,
    pub filter: Option<String>,
    pub case_sensitive: bool,
    pub within: Option<String>,
    pub resolve_bindings: bool,
    pub resolve_static: bool,
}

impl ResourcesQuery {
    pub fn validate(&self) -> Result<()> {
        if self
            .kind
            .as_deref()
            .is_some_and(|kind| !RESOURCE_KINDS.contains(&kind))
        {
            return Err(Error::new("INVALID_QUERY", "invalid JS resource kind"));
        }
        if self
            .resolution
            .as_deref()
            .is_some_and(|value| !matches!(value, "resolved" | "unresolved"))
        {
            return Err(Error::new(
                "INVALID_QUERY",
                "resource resolution must be resolved or unresolved",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct BindingProof {
    at: String,
    initializer_start: u32,
    initializer_end: u32,
}

#[derive(Serialize)]
pub(crate) struct Value {
    pub(crate) name: JsString,
    pub(crate) resolution: &'static str,
    pub(crate) unresolved_reason: Option<&'static str>,
    start: u32,
    end: u32,
    bindings: Vec<BindingProof>,
}

/// Keep string folding lossless in JS UTF-16. In particular, never route lone surrogates
/// through Rust String, and never invoke a user/shadowable string helper to guess a URL.
struct Strings<'b, 'a> {
    steps: usize,
    bindings: Option<&'b Bindings<'a>>,
    proofs: Vec<BindingProof>,
    decoders: Option<&'b crate::Decoders>,
    decoded: bool,
}

impl<'a> Strings<'_, 'a> {
    fn expression(
        &mut self,
        expression: &Expression<'a>,
        depth: usize,
    ) -> std::result::Result<JsString, &'static str> {
        if depth > FOLD_DEPTH || self.steps == 0 {
            return Err("fold_limit");
        }
        self.steps -= 1;
        if let Some(value) = self.decoders.and_then(|d| d.expression(expression)) {
            self.decoded = true;
            return Ok(value.value.clone());
        }
        match expression.get_inner_expression() {
            Expression::StringLiteral(literal) => {
                self.decoded(literal.value.as_str(), literal.lone_surrogates)
            }
            Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
                let mut value = self.expression(&binary.left, depth + 1)?;
                let right = self.expression(&binary.right, depth + 1)?;
                value
                    .append_bounded(&right, FOLD_UNITS)
                    .ok_or("fold_limit")?;
                Ok(value)
            }
            Expression::TemplateLiteral(template) => {
                let mut value = JsString::from("");
                for (index, quasi) in template.quasis.iter().enumerate() {
                    if self.steps == 0 {
                        return Err("fold_limit");
                    }
                    self.steps -= 1;
                    let cooked = quasi.value.cooked.as_ref().ok_or("not_closed_string")?;
                    let part = self.decoded(cooked.as_str(), quasi.lone_surrogates)?;
                    value
                        .append_bounded(&part, FOLD_UNITS)
                        .ok_or("fold_limit")?;
                    if let Some(expression) = template.expressions.get(index) {
                        let part = self.expression(expression, depth + 1)?;
                        value
                            .append_bounded(&part, FOLD_UNITS)
                            .ok_or("fold_limit")?;
                    }
                }
                Ok(value)
            }
            Expression::Identifier(id) if self.bindings.is_some() => {
                let (initializer, binding) = self.bindings.unwrap().initializer(id)?;
                let at = format!("symbol:{}:{}", binding.start, binding.end);
                if !self.proofs.iter().any(|p| p.at == at) {
                    self.proofs.push(BindingProof {
                        at,
                        initializer_start: initializer.span().start,
                        initializer_end: initializer.span().end,
                    });
                }
                self.expression(initializer, depth + 1)
            }
            _ => Err("not_closed_string"),
        }
    }

    fn decoded(&self, text: &str, lone: bool) -> std::result::Result<JsString, &'static str> {
        // Preflight before decoding/allocating; the UTF-16 size check also bounds ASCII.
        if text.len() > FOLD_UNITS * 8 {
            return Err("fold_limit");
        }
        let value = JsString::decoded(text, lone);
        if value.len_utf16() > FOLD_UNITS {
            Err("fold_limit")
        } else {
            Ok(value)
        }
    }
}

fn literal(literal: &StringLiteral<'_>) -> Value {
    Value {
        name: JsString::decoded(literal.value.as_str(), literal.lone_surrogates),
        resolution: "literal",
        unresolved_reason: None,
        start: literal.span.start,
        end: literal.span.end,
        bindings: Vec::new(),
    }
}

pub(crate) fn value(
    source: &Source,
    expression: &Expression<'_>,
    span: Span,
    forced: Option<&'static str>,
) -> Value {
    value_with_bindings(source, expression, span, forced, None, None)
}

fn value_with_bindings<'a>(
    source: &Source,
    expression: &Expression<'a>,
    span: Span,
    forced: Option<&'static str>,
    bindings: Option<&Bindings<'a>>,
    decoders: Option<&crate::Decoders>,
) -> Value {
    let inner = expression.get_inner_expression();
    if forced.is_none() {
        if let Expression::StringLiteral(string) = inner {
            let mut value = literal(string);
            value.start = span.start;
            value.end = span.end;
            return value;
        }
        if let Expression::TemplateLiteral(template) = inner
            && template.expressions.is_empty()
            && let Some(quasi) = template.quasis.first()
            && let Some(cooked) = &quasi.value.cooked
        {
            return Value {
                name: JsString::decoded(cooked.as_str(), quasi.lone_surrogates),
                resolution: "literal",
                unresolved_reason: None,
                start: span.start,
                end: span.end,
                bindings: Vec::new(),
            };
        }
    }
    let mut strings = Strings {
        steps: FOLD_STEPS,
        bindings,
        proofs: Vec::new(),
        decoders,
        decoded: false,
    };
    let result = forced.map_or_else(|| strings.expression(expression, 0), Err);
    let (name, resolution, reason) = match result {
        Ok(name) => (
            name,
            if strings.decoded {
                "decoder"
            } else if strings.proofs.is_empty() {
                "static"
            } else {
                "binding"
            },
            None,
        ),
        Err(reason) => (
            JsString::from(&source.text[span.start as usize..span.end as usize]),
            "unresolved",
            Some(reason),
        ),
    };
    Value {
        name,
        resolution,
        unresolved_reason: reason,
        start: span.start,
        end: span.end,
        bindings: if resolution == "binding" {
            strings.proofs
        } else {
            Vec::new()
        },
    }
}

/// Static member spellings only. Receivers and optional chains are not evaluated.
fn member<'e, 'a>(expression: &'e Expression<'a>) -> Option<(&'e Expression<'a>, &'e str, bool)> {
    fn computed<'e, 'a>(
        member: &'e ComputedMemberExpression<'a>,
    ) -> Option<(&'e Expression<'a>, &'e str, bool)> {
        let Expression::StringLiteral(key) = member.expression.get_inner_expression() else {
            return None;
        };
        Some((&member.object, key.value.as_str(), member.optional))
    }
    match expression.get_inner_expression() {
        Expression::StaticMemberExpression(m) => {
            Some((&m.object, m.property.name.as_str(), m.optional))
        }
        Expression::ComputedMemberExpression(m) => computed(m),
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::StaticMemberExpression(m) => {
                Some((&m.object, m.property.name.as_str(), m.optional))
            }
            ChainElement::ComputedMemberExpression(m) => computed(m),
            _ => None,
        },
        _ => None,
    }
}

fn path(expression: &Expression<'_>) -> Option<(String, bool)> {
    let mut expression = expression;
    let mut names = Vec::new();
    let mut optional = false;
    for _ in 0..4 {
        if let Expression::Identifier(id) = expression.get_inner_expression() {
            names.push(id.name.as_str());
            names.reverse();
            return Some((names.join("."), optional));
        }
        let (object, property, chain) = member(expression)?;
        names.push(property);
        optional |= chain;
        expression = object;
    }
    None
}

fn global_name(name: &str) -> &str {
    ["globalThis.", "window.", "self."]
        .into_iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
}

fn attribute(name: &str) -> Option<&'static str> {
    match name {
        "src" => Some("src"),
        "href" => Some("href"),
        _ => None,
    }
}

fn expression_attribute(expression: &Expression<'_>) -> Option<&'static str> {
    member(expression).and_then(|(_, name, _)| attribute(name))
}

fn target_attribute(target: &AssignmentTarget<'_>) -> Option<&'static str> {
    match target {
        AssignmentTarget::StaticMemberExpression(m) => attribute(m.property.name.as_str()),
        AssignmentTarget::ComputedMemberExpression(m) => {
            match m.expression.get_inner_expression() {
                Expression::StringLiteral(key) => attribute(key.value.as_str()),
                _ => None,
            }
        }
        AssignmentTarget::TSAsExpression(e) => expression_attribute(&e.expression),
        AssignmentTarget::TSSatisfiesExpression(e) => expression_attribute(&e.expression),
        AssignmentTarget::TSNonNullExpression(e) => expression_attribute(&e.expression),
        AssignmentTarget::TSTypeAssertion(e) => expression_attribute(&e.expression),
        _ => None,
    }
}

#[derive(Serialize)]
struct Base {
    kind: &'static str,
    expression: Option<Value>,
}

impl Base {
    fn context(kind: &'static str) -> Self {
        Self {
            kind,
            expression: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Site {
    kind: &'static str,
    syntax_kind: &'static str,
    span: Span,
    optional: bool,
    type_only: bool,
    argument_index: Option<usize>,
    spread: bool,
    attribute: Option<&'static str>,
    operator: Option<&'static str>,
}

impl Site {
    fn new(kind: &'static str, syntax_kind: &'static str, span: Span) -> Self {
        Self {
            kind,
            syntax_kind,
            span,
            optional: false,
            type_only: false,
            argument_index: None,
            spread: false,
            attribute: None,
            operator: None,
        }
    }
}

pub struct Resource {
    site: Site,
    value: Value,
    base: Base,
    parent_id: String,
    context_id: Option<String>,
}

impl Resource {
    pub fn unresolved(&self) -> bool {
        self.value.resolution == "unresolved"
    }

    pub fn envelope(&self) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            kind: &'static str,
            syntax_kind: &'static str,
            name: &'a JsString,
            resolution: &'static str,
            unresolved_reason: Option<&'static str>,
            site_start: u32,
            site_end: u32,
            argument_index: Option<usize>,
            spread: bool,
            optional: bool,
            type_only: bool,
            attribute: Option<&'static str>,
            operator: Option<&'static str>,
            base: &'a Base,
            bindings: &'a [BindingProof],
        }
        crate::envelope::body(
            "resource",
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.parent_id),
                context_at: self.context_id.as_deref(),
                start: Some(self.value.start as usize),
                end: Some(self.value.end as usize),
                ..Default::default()
            },
            Detail {
                kind: self.site.kind,
                syntax_kind: self.site.syntax_kind,
                name: &self.value.name,
                resolution: self.value.resolution,
                unresolved_reason: self.value.unresolved_reason,
                site_start: self.site.span.start,
                site_end: self.site.span.end,
                argument_index: self.site.argument_index,
                spread: self.site.spread,
                optional: self.site.optional,
                type_only: self.site.type_only,
                attribute: self.site.attribute,
                operator: self.site.operator,
                base: &self.base,
                bindings: &self.value.bindings,
            },
        )
    }
}

struct Visitor<'a, 's> {
    source: &'s Source,
    query: &'a ResourcesQuery,
    bindings: Option<&'a Bindings<'s>>,
    decoders: Option<&'a crate::Decoders>,
    locations: Locations<'a, 's>,
    contexts: Contexts,
    region: Span,
    records: Vec<Resource>,
}

impl<'s> Visitor<'_, 's> {
    fn value(
        &self,
        expression: &Expression<'s>,
        span: Span,
        forced: Option<&'static str>,
    ) -> Value {
        value_with_bindings(
            self.source,
            expression,
            span,
            forced,
            self.bindings,
            self.decoders,
        )
    }

    fn push(&mut self, site: Site, value: Value, base: Base) {
        if site.span.start < self.region.start
            || site.span.end > self.region.end
            || self
                .query
                .kind
                .as_deref()
                .is_some_and(|kind| kind != site.kind)
            || self.query.resolution.as_deref().is_some_and(|wanted| {
                (wanted == "unresolved") != (value.resolution == "unresolved")
            })
            || self.query.filter.as_ref().is_some_and(|needle| {
                !value
                    .name
                    .contains_filter(needle, self.query.case_sensitive)
            })
        {
            return;
        }
        let location = self.locations.get(site.span);
        self.records.push(Resource {
            site,
            value,
            base,
            parent_id: location.entity_id,
            context_id: self.contexts.current_id(self.source),
        });
    }

    fn import(
        &mut self,
        syntax: &'static str,
        span: Span,
        source: &StringLiteral<'_>,
        kind: ImportOrExportKind,
        base: &'static str,
    ) {
        let mut site = Site::new("module", syntax, span);
        site.type_only = kind == ImportOrExportKind::Type;
        self.push(site, literal(source), Base::context(base));
    }

    fn argument(&mut self, mut site: Site, args: &[Argument<'s>], index: usize, base: Base) {
        let Some(arg) = args.get(index) else { return };
        site.argument_index = Some(index);
        let (expression, forced) = match arg {
            Argument::SpreadElement(spread) => {
                site.spread = true;
                (&spread.argument, Some("spread_argument"))
            }
            arg => {
                let Some(expression) = arg.as_expression() else {
                    return;
                };
                (expression, None)
            }
        };
        self.push(site, self.value(expression, arg.span(), forced), base);
    }

    fn call(&mut self, call: &CallExpression<'s>) {
        let named = path(&call.callee);
        let (name, chain) = named
            .as_ref()
            .map_or(("", false), |(name, chain)| (name.as_str(), *chain));
        let global = global_name(name);
        let matched = match global {
            "fetch" => Some(("request", "fetch", "environment")),
            "navigator.sendBeacon" => Some(("request", "send_beacon", "environment")),
            "navigator.serviceWorker.register" => Some(("worker", "service_worker", "environment")),
            "importScripts" => Some(("script", "import_scripts", "worker")),
            _ if matches!(name, "require" | "module.require") => {
                Some(("module", "require", "commonjs"))
            }
            _ if name == "require.resolve" => Some(("module", "require_resolve", "commonjs")),
            _ => None,
        };
        if let Some((kind, syntax, base)) = matched {
            let mut site = Site::new(kind, syntax, call.span);
            site.optional = call.optional || chain;
            let count = if syntax == "import_scripts" {
                call.arguments.len()
            } else {
                1
            };
            for index in 0..count {
                self.argument(site, &call.arguments, index, Base::context(base));
            }
        } else if let Some((_, "setAttribute", optional)) = member(&call.callee)
            && let Some(first) = call.arguments.first().and_then(Argument::as_expression)
        {
            let name = value(self.source, first, first.span(), None);
            let attribute = ["src", "href"]
                .into_iter()
                .find(|key| name.resolution != "unresolved" && name.name.equals_str(key));
            if let Some(attribute) = attribute {
                let mut site = Site::new("attribute", "set_attribute", call.span);
                site.optional = call.optional || optional || chain;
                site.attribute = Some(attribute);
                self.argument(site, &call.arguments, 1, Base::context("environment"));
            }
        }
    }

    fn new_expression(&mut self, expression: &NewExpression<'s>) {
        let Some((name, _)) = path(&expression.callee) else {
            return;
        };
        let (kind, syntax) = match global_name(&name) {
            "Worker" => ("worker", "worker"),
            "SharedWorker" => ("worker", "shared_worker"),
            "URL" => ("url", "url"),
            "WebSocket" => ("request", "websocket"),
            "EventSource" => ("request", "event_source"),
            _ => return,
        };
        let mut base = Base::context(if kind == "url" { "none" } else { "environment" });
        if kind == "url" {
            if expression
                .arguments
                .first()
                .is_some_and(|arg| matches!(arg, Argument::SpreadElement(_)))
            {
                base = Base::context("unknown");
            } else if let Some(arg) = expression.arguments.get(1) {
                if let Some(second) = arg.as_expression() {
                    let meta = member(second).is_some_and(|(object, property, _)| {
                        property == "url"
                            && matches!(object.get_inner_expression(), Expression::ImportMeta(_))
                    });
                    base = Base {
                        kind: if meta { "import_meta_url" } else { "explicit" },
                        expression: Some(self.value(second, second.span(), None)),
                    };
                } else if let Argument::SpreadElement(spread) = arg {
                    base = Base {
                        kind: "unknown",
                        expression: Some(self.value(
                            &spread.argument,
                            spread.span,
                            Some("spread_argument"),
                        )),
                    };
                }
            }
        }
        self.argument(
            Site::new(kind, syntax, expression.span),
            &expression.arguments,
            0,
            base,
        );
    }
}

impl<'s> Visit<'s> for Visitor<'_, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
        self.contexts.enter(node, 0);
        match node {
            AstKind::ImportDeclaration(declaration) => self.import(
                "import",
                declaration.span,
                &declaration.source,
                declaration.import_kind,
                "module",
            ),
            AstKind::ExportFromDeclaration(declaration) => self.import(
                "export_from",
                declaration.span,
                &declaration.source,
                declaration.export_kind,
                "module",
            ),
            AstKind::ExportAllDeclaration(declaration) => self.import(
                "export_all",
                declaration.span,
                &declaration.source,
                declaration.export_kind,
                "module",
            ),
            AstKind::TSImportEqualsDeclaration(declaration) => {
                if let TSModuleReference::ExternalModuleReference(reference) =
                    &declaration.module_reference
                {
                    self.import(
                        "import_equals",
                        declaration.span,
                        &reference.expression,
                        declaration.import_kind,
                        "commonjs",
                    );
                }
            }
            AstKind::ImportExpression(import) => self.push(
                Site::new("module", "dynamic_import", import.span),
                self.value(&import.source, import.source.span(), None),
                Base::context("module"),
            ),
            AstKind::CallExpression(call) => self.call(call),
            AstKind::NewExpression(expression) => self.new_expression(expression),
            AstKind::AssignmentExpression(assignment) => {
                if let Some(attribute) = target_attribute(&assignment.left) {
                    let mut site = Site::new("attribute", "member_assignment", assignment.span);
                    site.attribute = Some(attribute);
                    site.operator = Some(assignment.operator.as_str());
                    let forced = (assignment.operator != AssignmentOperator::Assign)
                        .then_some("compound_assignment");
                    self.push(
                        site,
                        self.value(&assignment.right, assignment.right.span(), forced),
                        Base::context("environment"),
                    );
                }
            }
            _ => {}
        }
    }

    fn leave_node(&mut self, _node: AstKind<'s>) {
        self.contexts.leave();
    }
}

pub fn run<'s>(
    program: &'s Program<'s>,
    source: &'s Source,
    collector: &Collector<'s>,
    query: &ResourcesQuery,
    limits: rxd_core::Limits,
    perf: &mut Perf,
) -> Result<Vec<Resource>> {
    let region = match query.within.as_deref() {
        Some(id) => {
            let index = collector.by_id(id)?;
            let entity = &collector.entities[index].entity;
            Span::new(entity.start, entity.end)
        }
        None => Span::new(0, source.text.len() as u32),
    };
    let bindings = query
        .resolve_bindings
        .then(|| Bindings::build(program, limits, perf))
        .transpose()?;
    perf.measure("query", || {
        let mut visitor = Visitor {
            bindings: bindings.as_ref(),
            decoders: collector.decoders(),
            source,
            query,
            locations: Locations::new(collector),
            contexts: Contexts::new(Target::Matches),
            region,
            records: Vec::new(),
        };
        visitor.visit_program(program);
        visitor.records.sort_by_key(|record| {
            (
                record.value.start,
                record.value.end,
                record.site.span.start,
                record.site.argument_index,
            )
        });
        Ok(visitor.records)
    })
}
