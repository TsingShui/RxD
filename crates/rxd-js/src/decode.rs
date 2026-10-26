//! Static string-table recovery with lexical identities and exact, closed decoder shapes.
//! Input functions are matched, never executed; escapes and unsupported mutations reject a table.

use std::{
    collections::{HashMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind, panic_any, resume_unwind},
};

use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::Visit;
use oxc_semantic::{NodeId, Semantic, SymbolId};
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, UnaryOperator};
use rxd_core::{Error, Limits, Result, perf::Perf};

use crate::{lexical, static_eval, strings::JsString};

const MAX_DEPTH: usize = 32;
const GLOBAL_NAMES: [&str; 4] = ["globalThis", "global", "window", "self"];
const MAX_EXPANSION: usize = 64 * 1024;

#[derive(Default)]
pub struct Decoders {
    strings: HashMap<(u32, u32), JsString>,
}
impl Decoders {
    pub fn build<'a>(program: &'a Program<'a>, limits: Limits, perf: &mut Perf) -> Result<Self> {
        let mut scan = Scan {
            depth: 0,
            limit: limits.ast_depth,
            functions: Vec::new(),
            calls: Vec::new(),
        };
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| scan.visit_program(program))) {
            return match payload.downcast::<Error>() {
                Ok(error) => Err(*error),
                Err(payload) => resume_unwind(payload),
            };
        }
        // Ordinary files without a possible table never pay for semantic indexing.
        if !scan.functions.iter().any(|f| factory_array(f).is_some()) {
            return Ok(Self::default());
        }
        let semantic = lexical::build(program, limits, perf)?;
        if lexical::is_dynamic(&semantic) || dynamic_capabilities(&semantic) {
            return Ok(Self::default());
        }
        let mut readiness = Readiness::default();
        let mut result = Self::default();
        let mut retained = 0usize;
        let mut factories = HashMap::new();
        for function in &scan.functions {
            let Some(array) = factory_array(function) else {
                continue;
            };
            let Some(symbol) = function.id.as_ref().and_then(|id| id.symbol_id.get()) else {
                continue;
            };
            if !semantic.scoping().symbol_redeclarations(symbol).is_empty()
                || !factory_shape(function, array, symbol, &semantic)
            {
                continue;
            }
            retained = retained
                .checked_add(array.elements.len().saturating_mul(48))
                .ok_or_else(|| Error::new("RESOURCE_LIMIT", "JS decoder metadata overflow"))?;
            for element in &array.elements {
                if let ArrayExpressionElement::StringLiteral(s) = element {
                    retained = retained
                        .checked_add(s.value.len().saturating_mul(2))
                        .ok_or_else(|| {
                            Error::new("RESOURCE_LIMIT", "JS decoder metadata overflow")
                        })?;
                }
            }
            if retained > limits.metadata_bytes {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "JS decoder metadata budget exceeded",
                ));
            }
            let Some(values) = array
                .elements
                .iter()
                .map(|element| match element {
                    ArrayExpressionElement::StringLiteral(s) => {
                        Some(JsString::decoded(s.value.as_str(), s.lone_surrogates))
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            factories.insert(
                symbol,
                Factory {
                    span: function.span,
                    values,
                    cached: function
                        .body
                        .as_ref()
                        .is_some_and(|b| b.statements.len() == 3),
                },
            );
        }
        let mut decoders = HashMap::new();
        for function in &scan.functions {
            let Some(symbol) = function.id.as_ref().and_then(|id| id.symbol_id.get()) else {
                continue;
            };
            if let Some(decoder) = decoder_shape(function, &semantic, &factories)
                && !written(&semantic, symbol)
            {
                decoders.insert(symbol, decoder);
            }
        }
        // Aliases carry lexical identities and must be unwritten, initialized local consts.
        let mut aliases: Aliases = decoders
            .keys()
            .map(|s| {
                (
                    *s,
                    Alias {
                        target: *s,
                        ready: None,
                    },
                )
            })
            .collect();
        for _ in 0..MAX_DEPTH {
            let mut added = false;
            for node in semantic.nodes().iter() {
                let AstKind::VariableDeclarator(d) = node.kind() else {
                    continue;
                };
                let AstKind::VariableDeclaration(parent) = semantic.nodes().parent_kind(node.id())
                else {
                    continue;
                };
                if parent.kind != VariableDeclarationKind::Const {
                    continue;
                }
                let (Some(id), Some(Expression::Identifier(init))) =
                    (d.id.get_binding_identifier(), d.init.as_ref())
                else {
                    continue;
                };
                let Some(symbol) = id.symbol_id.get() else {
                    continue;
                };
                if aliases.contains_key(&symbol) || written(&semantic, symbol) {
                    continue;
                }
                let Some(alias) = symbol_ref(init, &semantic)
                    .and_then(|s| aliases.get(&s))
                    .copied()
                else {
                    continue;
                };
                if alias.ready.is_some_and(|ready| {
                    !ready_for_use(init.node_id.get(), ready, &semantic, &mut readiness, 0)
                }) {
                    continue;
                }
                let Some(ready) = declaration_ready(d.node_id.get(), d.span.end, &semantic) else {
                    continue;
                };
                retained = retained
                    .checked_add(96)
                    .ok_or_else(|| Error::new("RESOURCE_LIMIT", "JS decoder metadata overflow"))?;
                if retained > limits.metadata_bytes {
                    return Err(Error::new(
                        "RESOURCE_LIMIT",
                        "JS decoder metadata budget exceeded",
                    ));
                }
                aliases.insert(
                    symbol,
                    Alias {
                        target: alias.target,
                        ready: Some(ready),
                    },
                );
                added = true;
            }
            if !added {
                break;
            }
        }
        let mut validations = Vec::new();
        let mut regions = Vec::new();
        let mut initialized = HashSet::new();
        let mut order: Vec<_> = factories.keys().copied().collect();
        order.sort_by_key(|symbol|scan.calls.iter().filter(|call|call.arguments.iter().any(|arg|matches!(arg,Argument::Identifier(id) if symbol_ref(id,&semantic)==Some(*symbol)))).map(|call|call.span.start).min().unwrap_or(0));
        for symbol in &order {
            let Some(factory) = factories.get_mut(symbol) else {
                continue;
            };
            let own: HashMap<_, _> = decoders
                .iter()
                .filter(|(_, d)| d.factory == *symbol)
                .map(|(s, d)| (*s, d))
                .collect();
            if own.is_empty() {
                continue;
            }
            let rotations: Vec<_> = scan.calls.iter().filter(|call| call.arguments.iter().any(|arg| matches!(arg, Argument::Identifier(id) if symbol_ref(id, &semantic) == Some(*symbol)))).copied().collect();
            let rotation = match rotations.as_slice() {
                [] => None,
                [call] => rotation(
                    call,
                    *symbol,
                    factory,
                    &semantic,
                    &aliases,
                    &decoders,
                    &initialized,
                ),
                _ => continue,
            };
            if !rotations.is_empty() && (rotation.is_none() || !factory.cached) {
                continue;
            }
            let ready = rotation.as_ref().map(|(span, _)| Ready {
                end: span.end,
                owner: None,
                body: program.span,
            });
            let allowed: Vec<_> = own
                .values()
                .map(|d| d.span)
                .chain(rotation.as_ref().map(|(span, _)| *span))
                .chain([factory.span])
                .collect();
            if semantic
                .scoping()
                .get_resolved_references(*symbol)
                .any(|r| {
                    !allowed
                        .iter()
                        .any(|span| contains(*span, semantic.reference_span(r)))
                })
            {
                continue;
            }
            // No function escape, external property write or unsupported decoder reassignment.
            if own.iter().any(|(symbol, decoder)| {
                !safe_decoder_references(*symbol, decoder.span, &semantic, &aliases)
            }) || own
                .values()
                .any(|d| factory.values.iter().any(|v| !d.encoding.valid(v)))
            {
                continue;
            }
            if let Some((span, shift)) = rotation {
                factory.values.rotate_left(shift);
                initialized.insert(key(span));
            }
            regions.extend(allowed.iter().copied());
            readiness.cached = ready;
            readiness.proven.clear();
            let mut candidate = Self::default();
            for call in &scan.calls {
                if allowed.iter().any(|s| contains(*s, call.span)) {
                    continue;
                }
                let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
                    continue;
                };
                let Some(alias) = symbol_ref(callee, &semantic)
                    .and_then(|s| aliases.get(&s))
                    .copied()
                else {
                    continue;
                };
                let Some(decoder) = own.get(&alias.target) else {
                    continue;
                };
                if alias.ready.into_iter().chain(ready).any(|ready| {
                    !ready_for_use(call.node_id.get(), ready, &semantic, &mut readiness, 0)
                }) || call.optional
                    || call.arguments.len() != 1
                {
                    continue;
                }
                let Some(index) = call.arguments[0]
                    .as_expression()
                    .and_then(integer)
                    .and_then(|n| n.checked_sub(decoder.offset))
                    .and_then(|n| usize::try_from(n).ok())
                else {
                    continue;
                };
                let Some(value) = factory
                    .values
                    .get(index)
                    .and_then(|v| decoder.encoding.decode(v))
                else {
                    continue;
                };
                let bytes = value.len_utf16().saturating_mul(2).saturating_add(96);
                retained = retained
                    .checked_add(bytes)
                    .ok_or_else(|| Error::new("RESOURCE_LIMIT", "JS decoder metadata overflow"))?;
                if retained > limits.metadata_bytes {
                    return Err(Error::new(
                        "RESOURCE_LIMIT",
                        "JS decoder metadata budget exceeded",
                    ));
                }
                candidate.strings.insert(key(call.span), value);
            }
            result.strings.extend(candidate.strings);
            validations.push((*symbol, own.keys().copied().collect::<Vec<_>>()));
        }
        if opaque_capabilities(&semantic, &result, &regions)
            || validations
                .iter()
                .any(|(factory, symbols)| !private_globals(*factory, symbols, &semantic, &result))
        {
            return Ok(Self::default());
        }
        Ok(result)
    }
    pub fn call(&self, call: &CallExpression<'_>) -> Option<Resolved<'_>> {
        self.strings
            .get(&key(call.span))
            .map(|value| Resolved { value })
    }
    pub fn expression(&self, expression: &Expression<'_>) -> Option<Resolved<'_>> {
        self.at_span(expression.get_inner_expression().span())
    }
    pub fn at_span(&self, span: Span) -> Option<Resolved<'_>> {
        self.strings.get(&key(span)).map(|value| Resolved { value })
    }
}
pub struct Resolved<'a> {
    pub value: &'a JsString,
}
fn key(span: Span) -> (u32, u32) {
    (span.start, span.end)
}
fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}
fn symbol_ref(id: &IdentifierReference<'_>, semantic: &Semantic<'_>) -> Option<SymbolId> {
    semantic
        .scoping()
        .get_reference(id.reference_id.get()?)
        .symbol_id()
}
fn written(semantic: &Semantic<'_>, symbol: SymbolId) -> bool {
    !semantic.scoping().symbol_redeclarations(symbol).is_empty()
        || semantic
            .scoping()
            .get_resolved_references(symbol)
            .any(|r| r.is_write())
}
fn ref_is(expression: &Expression<'_>, symbol: SymbolId, semantic: &Semantic<'_>) -> bool {
    matches!(expression.get_inner_expression(), Expression::Identifier(id) if symbol_ref(id, semantic) == Some(symbol))
}
fn integer(expression: &Expression<'_>) -> Option<i64> {
    let value = match static_eval::evaluate(
        expression,
        static_eval::Limits {
            allow_intrinsics: false,
            ..Default::default()
        },
    )? {
        static_eval::Value::Number(n) => n,
        _ => return None,
    };
    (value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0)
        .then_some(value as i64)
}
struct Scan<'a> {
    depth: usize,
    limit: usize,
    functions: Vec<&'a Function<'a>>,
    calls: Vec<&'a CallExpression<'a>>,
}
impl<'a> Visit<'a> for Scan<'a> {
    fn enter_node(&mut self, node: AstKind<'a>) {
        self.depth += 1;
        if self.depth > self.limit {
            panic_any(Error::new("RESOURCE_LIMIT", "AST depth budget exceeded"));
        }
        match node {
            AstKind::Function(f) => self.functions.push(f),
            AstKind::CallExpression(c) => self.calls.push(c),
            _ => {}
        }
    }
    fn leave_node(&mut self, _: AstKind<'a>) {
        self.depth -= 1;
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Ready {
    end: u32,
    owner: Option<NodeId>,
    body: Span,
}
#[derive(Clone, Copy)]
struct Alias {
    target: SymbolId,
    ready: Option<Ready>,
}
type Aliases = HashMap<SymbolId, Alias>;
#[derive(Default)]
struct Readiness {
    active: HashSet<SymbolId>,
    proven: HashSet<SymbolId>,
    cached: Option<Ready>,
}
fn activation(node: NodeId, semantic: &Semantic<'_>) -> Option<NodeId> {
    semantic
        .nodes()
        .ancestors(node)
        .find(|n| {
            n.id() != node
                && matches!(
                    n.kind(),
                    AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
                )
        })
        .map(|n| n.id())
}
fn declaration_ready(node: NodeId, end: u32, semantic: &Semantic<'_>) -> Option<Ready> {
    let body = semantic
        .nodes()
        .ancestors(node)
        .find(|n| {
            matches!(
                n.kind(),
                AstKind::Program(_)
                    | AstKind::FunctionBody(_)
                    | AstKind::BlockStatement(_)
                    | AstKind::SwitchCase(_)
            )
        })?
        .span();
    Some(Ready {
        end,
        owner: activation(node, semantic),
        body,
    })
}
fn declaration_exported(node: NodeId, semantic: &Semantic<'_>) -> bool {
    let parent = semantic.nodes().parent_node(node);
    let parent = if matches!(parent.kind(), AstKind::VariableDeclaration(_)) {
        semantic.nodes().parent_node(parent.id())
    } else {
        parent
    };
    matches!(
        parent.kind(),
        AstKind::ExportDeclaration(_) | AstKind::ExportDefaultDeclaration(_)
    )
}
fn exported(function: &Function<'_>, semantic: &Semantic<'_>) -> bool {
    declaration_exported(function.node_id.get(), semantic)
        || function
            .id
            .as_ref()
            .and_then(|id| id.symbol_id.get())
            .is_some_and(|symbol| {
                semantic.scoping().get_resolved_references(symbol).any(|r| {
                    matches!(
                        semantic.nodes().parent_kind(r.node_id()),
                        AstKind::ExportSpecifier(_)
                    )
                })
            })
}
fn script_global(symbol: SymbolId, semantic: &Semantic<'_>) -> bool {
    !semantic.source_type().is_module()
        && semantic
            .nodes()
            .iter()
            .find(|n| matches!(n.kind(), AstKind::Program(_)))
            .is_some_and(|root| semantic.scoping().symbol_scope_id(symbol) == root.scope_id())
}
fn global_receiver(expression: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    global_root(expression, semantic)
        || matches!(
            expression.get_inner_expression(),
            Expression::ThisExpression(_)
        )
}
fn root_escape(id: &IdentifierReference<'_>, semantic: &Semantic<'_>) -> bool {
    if !matches!(
        id.name.as_str(),
        "globalThis" | "global" | "window" | "self"
    ) || symbol_ref(id, semantic).is_some()
    {
        return false;
    }
    !matches!(semantic.nodes().parent_kind(id.node_id.get()),AstKind::StaticMemberExpression(m) if m.object.span()==id.span)
        && !matches!(semantic.nodes().parent_kind(id.node_id.get()),AstKind::ComputedMemberExpression(m) if m.object.span()==id.span)
}
// Hoisted declarations require every external read/escape to follow initialization.
fn ready_for_use(
    node: NodeId,
    ready: Ready,
    semantic: &Semantic<'_>,
    seen: &mut Readiness,
    depth: usize,
) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let owner = activation(node, semantic);
    if owner == ready.owner {
        let span = semantic.nodes().get_node(node).span();
        return span.start >= ready.end && contains(ready.body, span);
    }
    let Some(owner) = owner else { return false };
    match semantic.nodes().kind(owner) {
        AstKind::Function(function) if function.is_declaration() => {
            let Some(symbol) = function.id.as_ref().and_then(|id| id.symbol_id.get()) else {
                return false;
            };
            if seen.cached == Some(ready) && seen.proven.contains(&symbol) {
                return true;
            }
            if exported(function, semantic)
                || written(semantic, symbol)
                || !seen.active.insert(symbol)
            {
                return false;
            }
            let result = semantic.scoping().get_resolved_references(symbol).all(|r| {
                contains(function.span, semantic.reference_span(r))
                    || ready_for_use(r.node_id(), ready, semantic, seen, depth + 1)
            }) && (!script_global(symbol, semantic)
                || semantic.nodes().iter().all(|node| {
                    if contains(function.span, node.span()) {
                        return true;
                    }
                    let name = semantic.scoping().symbol_name(symbol);
                    let access = match node.kind() {
                        AstKind::StaticMemberExpression(m) => {
                            global_receiver(&m.object, semantic) && m.property.name == name
                        }
                        AstKind::ComputedMemberExpression(m) => {
                            global_receiver(&m.object, semantic)
                                && property_may_touch(&m.expression, name, true)
                        }
                        AstKind::IdentifierReference(id) => root_escape(id, semantic),
                        AstKind::ThisExpression(t) => this_escape(t.node_id.get(), semantic),
                        _ => false,
                    };
                    !access || ready_for_use(node.id(), ready, semantic, seen, depth + 1)
                }));
            seen.active.remove(&symbol);
            if result && seen.cached == Some(ready) {
                seen.proven.insert(symbol);
            }
            result
        }
        AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => {
            ready_for_use(owner, ready, semantic, seen, depth + 1)
        }
        _ => false,
    }
}
fn this_escape(node: NodeId, semantic: &Semantic<'_>) -> bool {
    let span = semantic.nodes().get_node(node).span();
    !matches!(semantic.nodes().parent_kind(node),AstKind::StaticMemberExpression(m) if m.object.span()==span)
        && !matches!(semantic.nodes().parent_kind(node),AstKind::ComputedMemberExpression(m) if m.object.span()==span)
}
fn known_key(expression: &Expression<'_>, facts: &Decoders) -> Option<JsString> {
    static_eval::evaluate(
        expression,
        static_eval::Limits {
            allow_intrinsics: false,
            ..Default::default()
        },
    )
    .and_then(|v| v.string())
    .or_else(|| {
        facts
            .expression(expression)
            .map(|value| value.value.clone())
    })
}
fn dangerous_key(key: &JsString) -> bool {
    ["eval", "Function", "constructor"]
        .iter()
        .any(|name| key.equals_str(name))
}
fn dynamic_capabilities(semantic: &Semantic<'_>) -> bool {
    let empty = Decoders::default();
    semantic.nodes().iter().any(|node| match node.kind() {
        AstKind::IdentifierReference(id) => {
            matches!(id.name.as_str(), "eval" | "Function" | "Reflect")
                && symbol_ref(id, semantic).is_none()
        }
        AstKind::StaticMemberExpression(m) => matches!(
            m.property.name.as_str(),
            "eval" | "Function" | "constructor"
        ),
        AstKind::BindingProperty(p) => {
            if p.computed {
                p.key
                    .as_expression()
                    .and_then(|e| known_key(e, &empty))
                    .is_none_or(|key| dangerous_key(&key))
            } else {
                p.key
                    .static_name()
                    .is_some_and(|key| matches!(key.as_ref(), "eval" | "Function" | "constructor"))
            }
        }
        AstKind::ComputedMemberExpression(m) => {
            known_key(&m.expression, &empty).is_some_and(|key| dangerous_key(&key))
        }
        _ => false,
    })
}
fn opaque_capabilities(semantic: &Semantic<'_>, facts: &Decoders, allowed: &[Span]) -> bool {
    semantic.nodes().iter().any(|node| {
        if allowed.iter().any(|span| contains(*span, node.span())) {
            return false;
        }
        let AstKind::ComputedMemberExpression(m) = node.kind() else {
            return false;
        };
        known_key(&m.expression, facts).is_none_or(|key| dangerous_key(&key))
    })
}
fn private_globals(
    factory_symbol: SymbolId,
    decoders: &[SymbolId],
    semantic: &Semantic<'_>,
    facts: &Decoders,
) -> bool {
    let mut names: Vec<_> = std::iter::once(factory_symbol)
        .chain(decoders.iter().copied())
        .filter(|s| script_global(*s, semantic))
        .map(|s| semantic.scoping().symbol_name(s))
        .collect();
    if names.is_empty() {
        return true;
    }
    names.extend(GLOBAL_NAMES);
    semantic.nodes().iter().all(|node| match node.kind() {
        AstKind::IdentifierReference(id) => !root_escape(id, semantic),
        AstKind::ThisExpression(t) => !this_escape(t.node_id.get(), semantic),
        AstKind::StaticMemberExpression(m) if global_receiver(&m.object, semantic) => {
            !names.iter().any(|name| m.property.name == *name)
        }
        AstKind::ComputedMemberExpression(m) if global_receiver(&m.object, semantic) => {
            known_key(&m.expression, facts)
                .is_some_and(|key| !names.iter().any(|name| key.equals_str(name)))
        }
        _ => true,
    })
}
struct Factory {
    span: Span,
    values: Vec<JsString>,
    cached: bool,
}
struct Decoder {
    span: Span,
    factory: SymbolId,
    offset: i64,
    encoding: Encoding,
}
#[derive(Clone, Copy)]
enum Encoding {
    Plain,
    Base64,
}
impl Encoding {
    fn valid(self, value: &JsString) -> bool {
        match self {
            Self::Plain => true,
            Self::Base64 => base64(value).is_some(),
        }
    }
    fn decode(self, value: &JsString) -> Option<JsString> {
        match self {
            Self::Plain => Some(value.clone()),
            Self::Base64 => base64(value),
        }
    }
}
fn factory_array<'a>(function: &'a Function<'a>) -> Option<&'a ArrayExpression<'a>> {
    if !function.is_declaration()
        || function.r#async
        || function.generator
        || !function.params.items.is_empty()
        || function.params.rest.is_some()
    {
        return None;
    }
    let Statement::VariableDeclaration(decl) = function.body.as_ref()?.statements.first()? else {
        return None;
    };
    if decl.declarations.len() != 1 {
        return None;
    }
    let Expression::ArrayExpression(array) =
        decl.declarations[0].init.as_ref()?.get_inner_expression()
    else {
        return None;
    };
    (!array.elements.is_empty()).then_some(array)
}
fn factory_shape(
    function: &Function<'_>,
    array: &ArrayExpression<'_>,
    symbol: SymbolId,
    semantic: &Semantic<'_>,
) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    let Statement::VariableDeclaration(decl) = &body.statements[0] else {
        return false;
    };
    if decl.kind != VariableDeclarationKind::Const
        || exported(function, semantic)
        || !lexical::hoisted_function(function, semantic)
    {
        return false;
    }
    let Some(id) = decl.declarations[0].id.get_binding_identifier() else {
        return false;
    };
    let Some(array_symbol) = id.symbol_id.get() else {
        return false;
    };
    (match body.statements.as_slice() {
        [_, Statement::ReturnStatement(ret)] => ret
            .argument
            .as_ref()
            .is_some_and(|e| ref_is(e, array_symbol, semantic)),
        [
            _,
            Statement::ExpressionStatement(expr),
            Statement::ReturnStatement(ret),
        ] => {
            let Expression::AssignmentExpression(assign) = expr.expression.get_inner_expression()
            else {
                return false;
            };
            if assign.operator != AssignmentOperator::Assign {
                return false;
            }
            let AssignmentTarget::AssignmentTargetIdentifier(target) = &assign.left else {
                return false;
            };
            if symbol_ref(target, semantic) != Some(symbol) {
                return false;
            }
            let Expression::FunctionExpression(cached) = assign.right.get_inner_expression() else {
                return false;
            };
            if cached.id.is_some()
                || cached.r#async
                || cached.generator
                || !cached.params.items.is_empty()
                || cached.params.rest.is_some()
            {
                return false;
            }
            let Some(cached_body) = &cached.body else {
                return false;
            };
            let [Statement::ReturnStatement(r)] = cached_body.statements.as_slice() else {
                return false;
            };
            if !r
                .argument
                .as_ref()
                .is_some_and(|e| ref_is(e, array_symbol, semantic))
            {
                return false;
            }
            matches!(&ret.argument, Some(Expression::CallExpression(c)) if c.arguments.is_empty() && !c.optional && ref_is(&c.callee, symbol, semantic))
        }
        _ => false,
    }) && array
        .elements
        .iter()
        .all(|e| matches!(e, ArrayExpressionElement::StringLiteral(_)))
}
#[derive(Clone, Copy)]
enum Symbolic {
    Index { offset: i64, adjusted: bool },
    Table(SymbolId),
    Lookup(SymbolId, i64, Encoding),
}
fn decoder_shape(
    function: &Function<'_>,
    semantic: &Semantic<'_>,
    factories: &HashMap<SymbolId, Factory>,
) -> Option<Decoder> {
    if !function.is_declaration()
        || !lexical::hoisted_function(function, semantic)
        || function.r#async
        || function.generator
        || function.params.rest.is_some()
    {
        return None;
    }
    let parameter = function
        .params
        .items
        .first()?
        .pattern
        .get_binding_identifier()?
        .symbol_id
        .get()?;
    let mut parameters = HashSet::new();
    if function.params.items.iter().any(|p| {
        p.initializer.is_some()
            || p.pattern
                .get_binding_identifier()
                .and_then(|id| id.symbol_id.get())
                .is_none_or(|symbol| !parameters.insert(symbol))
    }) {
        return None;
    }
    let mut env = HashMap::from([(
        parameter,
        Symbolic::Index {
            offset: 0,
            adjusted: false,
        },
    )]);
    let statements = &function.body.as_ref()?.statements;
    let (last, setup) = statements.split_last()?;
    for statement in setup {
        match statement {
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if !matches!(
                        decl.kind,
                        VariableDeclarationKind::Const
                            | VariableDeclarationKind::Let
                            | VariableDeclarationKind::Var
                    ) {
                        return None;
                    }
                    let symbol = d.id.get_binding_identifier()?.symbol_id.get()?;
                    if written(semantic, symbol) {
                        return None;
                    }
                    let value = symbolic(d.init.as_ref()?, &env, semantic, factories, 0)?;
                    env.insert(symbol, value);
                }
            }
            Statement::ExpressionStatement(expr) => {
                let Expression::AssignmentExpression(assign) =
                    expr.expression.get_inner_expression()
                else {
                    return None;
                };
                let AssignmentTarget::AssignmentTargetIdentifier(target) = &assign.left else {
                    return None;
                };
                if symbol_ref(target, semantic) != Some(parameter)
                    || assign.operator != AssignmentOperator::Assign
                {
                    return None;
                }
                let value = symbolic(&assign.right, &env, semantic, factories, 0)?;
                if !matches!(value, Symbolic::Index { .. }) {
                    return None;
                }
                env.insert(parameter, value);
            }
            _ => return None,
        }
    }
    let Statement::ReturnStatement(ret) = last else {
        return None;
    };
    let Symbolic::Lookup(factory, offset, encoding) =
        symbolic(ret.argument.as_ref()?, &env, semantic, factories, 0)?
    else {
        return None;
    };
    Some(Decoder {
        span: function.span,
        factory,
        offset,
        encoding,
    })
}
fn symbolic(
    expression: &Expression<'_>,
    env: &HashMap<SymbolId, Symbolic>,
    semantic: &Semantic<'_>,
    factories: &HashMap<SymbolId, Factory>,
    depth: usize,
) -> Option<Symbolic> {
    if depth > MAX_DEPTH {
        return None;
    }
    match expression.get_inner_expression() {
        Expression::Identifier(id) => env.get(&symbol_ref(id, semantic)?).copied(),
        Expression::BinaryExpression(binary) => {
            let Symbolic::Index {
                offset,
                adjusted: false,
            } = symbolic(&binary.left, env, semantic, factories, depth + 1)?
            else {
                return None;
            };
            let amount = integer(&binary.right)?;
            let offset = match binary.operator {
                BinaryOperator::Subtraction => offset.checked_add(amount)?,
                BinaryOperator::Addition => offset.checked_sub(amount)?,
                _ => return None,
            };
            Some(Symbolic::Index {
                offset,
                adjusted: true,
            })
        }
        Expression::ComputedMemberExpression(member) if !member.optional => {
            let Symbolic::Table(factory) =
                symbolic(&member.object, env, semantic, factories, depth + 1)?
            else {
                return None;
            };
            let Symbolic::Index { offset, .. } =
                symbolic(&member.expression, env, semantic, factories, depth + 1)?
            else {
                return None;
            };
            Some(Symbolic::Lookup(factory, offset, Encoding::Plain))
        }
        Expression::CallExpression(call) if !call.optional => {
            let Expression::Identifier(id) = call.callee.get_inner_expression() else {
                return None;
            };
            if call.arguments.is_empty()
                && let Some(symbol) = symbol_ref(id, semantic)
                && factories.contains_key(&symbol)
            {
                return Some(Symbolic::Table(symbol));
            }
            if call.arguments.len() != 1
                || id.name != "atob"
                || symbol_ref(id, semantic).is_some()
                || !global_unmodified(semantic, "atob", None)
            {
                return None;
            }
            let Symbolic::Lookup(factory, offset, Encoding::Plain) = symbolic(
                call.arguments[0].as_expression()?,
                env,
                semantic,
                factories,
                depth + 1,
            )?
            else {
                return None;
            };
            Some(Symbolic::Lookup(factory, offset, Encoding::Base64))
        }
        _ => None,
    }
}
fn safe_decoder_references(
    symbol: SymbolId,
    decoder_span: Span,
    semantic: &Semantic<'_>,
    aliases: &Aliases,
) -> bool {
    aliases
        .iter()
        .filter(|(_, alias)| alias.target == symbol)
        .all(|(alias, _)| {
            !declaration_exported(semantic.symbol_declaration(*alias).id(), semantic)
                && semantic
                    .scoping()
                    .get_resolved_references(*alias)
                    .all(|reference| {
                        let span = semantic.reference_span(reference);
                        if contains(decoder_span, span) {
                            return true;
                        }
                        match semantic.nodes().parent_kind(reference.node_id()) {
                            AstKind::CallExpression(call) => {
                                call.callee.span() == span
                                    && !call.optional
                                    && call.arguments.len() == 1
                                    && call.arguments[0]
                                        .as_expression()
                                        .and_then(integer)
                                        .is_some()
                            }
                            AstKind::VariableDeclarator(d) => {
                                d.id.get_binding_identifier()
                                    .and_then(|id| id.symbol_id.get())
                                    .is_some_and(|s| aliases.contains_key(&s))
                            }
                            _ => false,
                        }
                    })
        })
}
fn global_root(expression: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    matches!(expression.get_inner_expression(),Expression::Identifier(id) if matches!(id.name.as_str(),"globalThis" | "global" | "window" | "self") && symbol_ref(id,semantic).is_none())
}
fn property_may_touch(expression: &Expression<'_>, name: &str, global: bool) -> bool {
    match static_eval::evaluate(
        expression,
        static_eval::Limits {
            allow_intrinsics: false,
            ..Default::default()
        },
    ) {
        Some(static_eval::Value::String(value)) => value.equals_str(name),
        Some(_) | None => global,
    }
}
fn runtime_dependency(node: AstKind<'_>, semantic: &Semantic<'_>) -> bool {
    match node {
        AstKind::ImportDeclaration(d) => d.import_kind != ImportOrExportKind::Type,
        AstKind::ExportFromDeclaration(d) => d.export_kind != ImportOrExportKind::Type,
        AstKind::ExportAllDeclaration(d) => d.export_kind != ImportOrExportKind::Type,
        AstKind::ImportExpression(_) => true,
        AstKind::CallExpression(c) => {
            matches!(c.callee.get_inner_expression(),Expression::Identifier(id) if id.name=="require" && symbol_ref(id,semantic).is_none())
        }
        _ => false,
    }
}
fn global_unmodified(semantic: &Semantic<'_>, name: &str, before: Option<u32>) -> bool {
    let initialized = before.and_then(|end| {
        semantic.nodes().iter().find_map(|n| {
            if let AstKind::Program(p) = n.kind() {
                Some(Ready {
                    end,
                    owner: None,
                    body: p.span,
                })
            } else {
                None
            }
        })
    });
    let mut readiness = Readiness {
        cached: initialized,
        ..Default::default()
    };
    semantic.nodes().iter().all(|node| {
        if runtime_dependency(node.kind(),semantic) {return false;}
        if let AstKind::BindingIdentifier(id)=node.kind() { return id.name != name; }
        if initialized.is_some_and(|ready|ready_for_use(node.id(),ready,semantic,&mut readiness,0)) { return true; }
        match node.kind() {
            AstKind::StaticMemberExpression(m) if global_receiver(&m.object,semantic) && GLOBAL_NAMES.contains(&m.property.name.as_str())=>false,
            AstKind::ComputedMemberExpression(m) if global_receiver(&m.object,semantic) && known_key(&m.expression,&Decoders::default()).is_some_and(|key|GLOBAL_NAMES.iter().any(|name|key.equals_str(name)))=>false,
            AstKind::ThisExpression(t) if this_escape(t.node_id.get(),semantic)=>false,
            AstKind::StaticMemberExpression(m) if name=="Array" && matches!(m.property.name.as_str(),"prototype" | "__proto__" | "getPrototypeOf") => false,
            AstKind::ComputedMemberExpression(m) if name=="Array" && ["prototype","__proto__","getPrototypeOf"].iter().any(|key|property_may_touch(&m.expression,key,false)) => false,
            AstKind::IdentifierReference(id) if name=="Array" && matches!(id.name.as_str(),"Object" | "Reflect") && symbol_ref(id,semantic).is_none() =>
                matches!(semantic.nodes().parent_kind(node.id()),AstKind::StaticMemberExpression(m) if m.object.span()==id.span) ||
                matches!(semantic.nodes().parent_kind(node.id()),AstKind::ComputedMemberExpression(m) if m.object.span()==id.span && static_eval::evaluate(&m.expression,static_eval::Limits{allow_intrinsics:false,..Default::default()}).is_some()),
            AstKind::IdentifierReference(id) if id.name == name && symbol_ref(id,semantic).is_none() =>
                matches!(semantic.nodes().parent_kind(node.id()),AstKind::CallExpression(c) if c.callee.span()==id.span),
            AstKind::IdentifierReference(id) if matches!(id.name.as_str(),"globalThis" | "global" | "window" | "self") && symbol_ref(id,semantic).is_none() =>
                matches!(semantic.nodes().parent_kind(node.id()),AstKind::StaticMemberExpression(m) if m.object.span()==id.span) ||
                matches!(semantic.nodes().parent_kind(node.id()),AstKind::ComputedMemberExpression(m) if m.object.span()==id.span),
            AstKind::StaticMemberExpression(m) if global_root(&m.object,semantic) && m.property.name==name =>
                matches!(semantic.nodes().parent_kind(node.id()),AstKind::CallExpression(c) if c.callee.span()==m.span),
            AstKind::ComputedMemberExpression(m) if global_root(&m.object,semantic) && property_may_touch(&m.expression,name,true) =>
                matches!(semantic.nodes().parent_kind(node.id()),AstKind::CallExpression(c) if c.callee.span()==m.span),
            AstKind::CallExpression(c) if member(&c.callee).is_some_and(|(_,p)|matches!(p,"defineProperty" | "defineProperties" | "set" | "assign" | "deleteProperty")) =>
                !c.arguments.iter().any(|a|a.as_expression().is_some_and(|e|touches(e,name) || global_root(e,semantic))),
            AstKind::UnaryExpression(u) if u.operator == UnaryOperator::Delete => !touches(&u.argument,name),
            AstKind::AssignmentExpression(a) => a.left.get_identifier_name()!=Some(name) && match &a.left {
                AssignmentTarget::StaticMemberExpression(m)=>m.property.name!=name && !touches(&m.object,name) && !(name=="Array" && matches!(m.property.name.as_str(),"push" | "shift")),
                AssignmentTarget::ComputedMemberExpression(m)=>!touches(&m.object,name) && !property_may_touch(&m.expression,name,global_receiver(&m.object,semantic)) && !(name=="Array" && ["push","shift"].iter().any(|key|property_may_touch(&m.expression,key,true))),
                _=>true,
            },
            AstKind::UpdateExpression(u) => u.argument.get_identifier_name()!=Some(name) && match &u.argument {
                SimpleAssignmentTarget::StaticMemberExpression(m)=>m.property.name!=name && !touches(&m.object,name),
                SimpleAssignmentTarget::ComputedMemberExpression(m)=>!touches(&m.object,name) && !property_may_touch(&m.expression,name,global_receiver(&m.object,semantic)),
                _=>true,
            },
            _=>true,
        }
    })
}
fn touches(expression: &Expression<'_>, name: &str) -> bool {
    match expression.get_inner_expression() {
        Expression::Identifier(id) => id.name == name,
        Expression::StringLiteral(s) => s.value == name,
        Expression::StaticMemberExpression(m) => {
            m.property.name == name || touches(&m.object, name)
        }
        Expression::ComputedMemberExpression(m) => {
            touches(&m.object, name)
                || matches!(m.expression.get_inner_expression(),Expression::StringLiteral(s) if s.value==name)
        }
        _ => false,
    }
}
fn pure_initializer(
    expression: &Expression<'_>,
    semantic: &Semantic<'_>,
    aliases: &Aliases,
) -> bool {
    if static_eval::evaluate(
        expression,
        static_eval::Limits {
            allow_intrinsics: false,
            ..Default::default()
        },
    )
    .is_some()
    {
        return true;
    }
    match expression.get_inner_expression() {
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_) => true,
        Expression::Identifier(id) => {
            symbol_ref(id, semantic).is_some_and(|s| aliases.contains_key(&s))
        }
        Expression::CallExpression(c) if !c.optional && c.arguments.len() == 1 => {
            matches!(c.callee.get_inner_expression(),Expression::Identifier(id) if symbol_ref(id,semantic).is_some_and(|s|aliases.contains_key(&s)))
                && c.arguments[0].as_expression().and_then(integer).is_some()
        }
        _ => false,
    }
}
fn closed_prefix(
    call: &CallExpression<'_>,
    semantic: &Semantic<'_>,
    aliases: &Aliases,
    initialized: &HashSet<(u32, u32)>,
) -> bool {
    let Some(program) = semantic.nodes().iter().find_map(|node| {
        if let AstKind::Program(p) = node.kind() {
            Some(p)
        } else {
            None
        }
    }) else {
        return false;
    };
    program
        .body
        .iter()
        .take_while(|statement| statement.span().end <= call.span.start)
        .all(|statement| match statement {
            Statement::FunctionDeclaration(_) | Statement::EmptyStatement(_) => true,
            Statement::VariableDeclaration(d) => {
                matches!(
                    d.kind,
                    VariableDeclarationKind::Const
                        | VariableDeclarationKind::Let
                        | VariableDeclarationKind::Var
                ) && d.declarations.iter().all(|decl| {
                    decl.id.get_binding_identifier().is_some()
                        && decl
                            .init
                            .as_ref()
                            .is_none_or(|e| pure_initializer(e, semantic, aliases))
                })
            }
            Statement::ExpressionStatement(e) => {
                initialized.contains(&key(e.expression.get_inner_expression().span()))
                    || pure_initializer(&e.expression, semantic, aliases)
            }
            _ => false,
        })
}
fn rotation<'a>(
    call: &'a CallExpression<'a>,
    factory_symbol: SymbolId,
    factory: &Factory,
    semantic: &Semantic<'a>,
    aliases: &Aliases,
    decoders: &HashMap<SymbolId, Decoder>,
    initialized: &HashSet<(u32, u32)>,
) -> Option<(Span, usize)> {
    if call.arguments.len() != 2
        || call.optional
        || !closed_prefix(call, semantic, aliases, initialized)
    {
        return None;
    }
    let Expression::FunctionExpression(function) = call.callee.get_inner_expression() else {
        return None;
    };
    // Rotation is a top-level initializer, never a deferred or conditional call.
    let mut node = semantic.nodes().get_node(call.node_id.get());
    loop {
        let parent = semantic.nodes().parent_node(node.id());
        match parent.kind() {
            AstKind::ParenthesizedExpression(_) | AstKind::ExpressionStatement(_) => node = parent,
            AstKind::Program(_) => break,
            _ => return None,
        }
    }
    if function.r#async
        || function.generator
        || function.params.items.len() != 2
        || function.params.rest.is_some()
    {
        return None;
    }
    let first = function.params.items[0]
        .pattern
        .get_binding_identifier()?
        .symbol_id
        .get()?;
    let target_symbol = function.params.items[1]
        .pattern
        .get_binding_identifier()?
        .symbol_id
        .get()?;
    if first == target_symbol {
        return None;
    }
    let target = integer(call.arguments[1].as_expression()?)? as f64;
    let body = function.body.as_ref()?;
    let (last, setup) = body.statements.split_last()?;
    let mut array = None;
    for statement in setup {
        let Statement::VariableDeclaration(decl) = statement else {
            return None;
        };
        if decl.kind != VariableDeclarationKind::Const {
            return None;
        }
        for d in &decl.declarations {
            let id = d.id.get_binding_identifier()?.symbol_id.get()?;
            let init = d.init.as_ref()?.get_inner_expression();
            match init {
                Expression::Identifier(init)
                    if symbol_ref(init, semantic).is_some_and(|s| aliases.contains_key(&s)) => {}
                Expression::CallExpression(c)
                    if c.arguments.is_empty() && ref_is(&c.callee, first, semantic) =>
                {
                    if array.replace(id).is_some() {
                        return None;
                    }
                }
                _ => return None,
            }
            if written(semantic, id) {
                return None;
            }
        }
    }
    let Statement::WhileStatement(loop_) = last else {
        return None;
    };
    if !matches!(
        static_eval::evaluate(&loop_.test, Default::default()),
        Some(static_eval::Value::Bool(true))
    ) {
        return None;
    }
    let Statement::BlockStatement(block) = &loop_.body else {
        return None;
    };
    let [Statement::TryStatement(try_)] = block.body.as_slice() else {
        return None;
    };
    if try_.finalizer.is_some() {
        return None;
    }
    let [
        Statement::VariableDeclaration(decl),
        Statement::IfStatement(if_),
    ] = try_.block.body.as_slice()
    else {
        return None;
    };
    if decl.kind != VariableDeclarationKind::Const || decl.declarations.len() != 1 {
        return None;
    }
    let check = &decl.declarations[0];
    let check_symbol = check.id.get_binding_identifier()?.symbol_id.get()?;
    let Expression::BinaryExpression(test) = if_.test.get_inner_expression() else {
        return None;
    };
    if test.operator != BinaryOperator::StrictEquality
        || !ref_is(&test.left, check_symbol, semantic)
        || !ref_is(&test.right, target_symbol, semantic)
        || !matches!(&if_.consequent, Statement::BreakStatement(b) if b.label.is_none())
    {
        return None;
    }
    if !rotation_step(if_.alternate.as_ref()?, array, first, semantic) {
        return None;
    }
    let handler = try_.handler.as_ref()?;
    let [step] = handler.body.body.as_slice() else {
        return None;
    };
    if !rotation_step(step, array, first, semantic) {
        return None;
    }
    if !global_unmodified(semantic, "parseInt", Some(call.span.end))
        || !global_unmodified(semantic, "Array", Some(call.span.end))
    {
        return None;
    }
    let mut solution = None;
    for shift in 0..factory.values.len() {
        let mut steps = 0;
        let value = checksum(
            check.init.as_ref()?,
            factory_symbol,
            &factory.values,
            shift,
            semantic,
            aliases,
            decoders,
            &mut steps,
            0,
        )?;
        if value == target && solution.replace(shift).is_some() {
            return None;
        }
    }
    Some((call.span, solution?))
}
fn rotation_step(
    statement: &Statement<'_>,
    array: Option<SymbolId>,
    factory_parameter: SymbolId,
    semantic: &Semantic<'_>,
) -> bool {
    let Statement::ExpressionStatement(statement) = statement else {
        return false;
    };
    let Expression::CallExpression(push) = statement.expression.get_inner_expression() else {
        return false;
    };
    if push.arguments.len() != 1 || push.optional {
        return false;
    }
    let Some((object, name)) = member(&push.callee) else {
        return false;
    };
    if name != "push" || !rotation_array(object, array, factory_parameter, semantic) {
        return false;
    }
    let Some(Expression::CallExpression(shift)) = push.arguments[0]
        .as_expression()
        .map(Expression::get_inner_expression)
    else {
        return false;
    };
    if !shift.arguments.is_empty() || shift.optional {
        return false;
    }
    let Some((object, name)) = member(&shift.callee) else {
        return false;
    };
    name == "shift" && rotation_array(object, array, factory_parameter, semantic)
}
fn rotation_array(
    expression: &Expression<'_>,
    array: Option<SymbolId>,
    parameter: SymbolId,
    semantic: &Semantic<'_>,
) -> bool {
    match expression.get_inner_expression() {
        Expression::Identifier(id) => array.is_some() && symbol_ref(id, semantic) == array,
        Expression::CallExpression(call) => {
            !call.optional && call.arguments.is_empty() && ref_is(&call.callee, parameter, semantic)
        }
        _ => false,
    }
}
fn member<'a>(expression: &'a Expression<'a>) -> Option<(&'a Expression<'a>, &'a str)> {
    match expression.get_inner_expression() {
        Expression::StaticMemberExpression(m) if !m.optional => {
            Some((&m.object, m.property.name.as_str()))
        }
        Expression::ComputedMemberExpression(m) if !m.optional => {
            match m.expression.get_inner_expression() {
                Expression::StringLiteral(s) => Some((&m.object, s.value.as_str())),
                _ => None,
            }
        }
        _ => None,
    }
}
#[allow(clippy::too_many_arguments)]
fn checksum(
    expression: &Expression<'_>,
    factory: SymbolId,
    values: &[JsString],
    shift: usize,
    semantic: &Semantic<'_>,
    aliases: &Aliases,
    decoders: &HashMap<SymbolId, Decoder>,
    steps: &mut usize,
    depth: usize,
) -> Option<f64> {
    if depth > MAX_DEPTH || *steps >= 512 {
        return None;
    }
    *steps += 1;
    let mut number = |e| {
        checksum(
            e,
            factory,
            values,
            shift,
            semantic,
            aliases,
            decoders,
            steps,
            depth + 1,
        )
    };
    match expression.get_inner_expression() {
        Expression::NumericLiteral(n) => Some(n.value),
        Expression::UnaryExpression(u) => {
            let value = number(&u.argument)?;
            match u.operator {
                UnaryOperator::UnaryPlus => Some(value),
                UnaryOperator::UnaryNegation => Some(-value),
                _ => None,
            }
        }
        Expression::BinaryExpression(b) => {
            let left = number(&b.left)?;
            let right = number(&b.right)?;
            match b.operator {
                BinaryOperator::Addition => Some(left + right),
                BinaryOperator::Subtraction => Some(left - right),
                BinaryOperator::Multiplication => Some(left * right),
                BinaryOperator::Division => Some(left / right),
                _ => None,
            }
        }
        Expression::CallExpression(c)
            if !c.optional && c.arguments.len() == 1 && c.callee.is_specific_id("parseInt") =>
        {
            let Expression::Identifier(parse) = c.callee.get_inner_expression() else {
                return None;
            };
            if symbol_ref(parse, semantic).is_some() {
                return None;
            }
            let Expression::CallExpression(lookup) =
                c.arguments[0].as_expression()?.get_inner_expression()
            else {
                return None;
            };
            let Expression::Identifier(callee) = lookup.callee.get_inner_expression() else {
                return None;
            };
            let alias = aliases.get(&symbol_ref(callee, semantic)?)?;
            if alias.ready.is_some_and(|ready| {
                !ready_for_use(
                    lookup.node_id.get(),
                    ready,
                    semantic,
                    &mut Readiness::default(),
                    0,
                )
            }) {
                return None;
            }
            let decoder = decoders.get(&alias.target)?;
            if decoder.factory != factory || lookup.arguments.len() != 1 || lookup.optional {
                return None;
            }
            let index = usize::try_from(
                integer(lookup.arguments[0].as_expression()?)?.checked_sub(decoder.offset)?,
            )
            .ok()?;
            if index >= values.len() {
                return Some(f64::NAN);
            }
            let value = decoder
                .encoding
                .decode(&values[(index + shift) % values.len()])?;
            parse_int(&value)
        }
        _ => None,
    }
}
fn parse_int(value: &JsString) -> Option<f64> {
    let mut digits = value.units();
    while digits.first().is_some_and(|u| matches!(*u, 9..=13 | 32 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF)) {
        digits = &digits[1..];
    }
    let sign = if digits.first() == Some(&45) {
        -1.0
    } else {
        1.0
    };
    if matches!(digits.first(), Some(43 | 45)) {
        digits = &digits[1..];
    }
    let radix = if matches!(digits, [48, 88 | 120, ..]) {
        digits = &digits[2..];
        16
    } else {
        10
    };
    let mut result = 0u64;
    let mut count = 0;
    for &unit in digits {
        let digit = match unit {
            48..=57 => unit - 48,
            65..=70 => unit - 65 + 10,
            97..=102 => unit - 97 + 10,
            _ => break,
        };
        if digit >= radix {
            break;
        }
        result = result
            .checked_mul(u64::from(radix))?
            .checked_add(u64::from(digit))?;
        if result > 9_007_199_254_740_991 {
            return None;
        }
        count += 1;
    }
    Some(if count == 0 {
        f64::NAN
    } else {
        result as f64 * sign
    })
}
fn base64(value: &JsString) -> Option<JsString> {
    // atob yields Latin-1 code units, not a UTF-8 string.
    let mut input: Vec<u16> = value
        .units()
        .iter()
        .copied()
        .filter(|u| !matches!(*u, 9 | 10 | 12 | 13 | 32))
        .collect();
    if input.len() > MAX_EXPANSION * 2 {
        return None;
    }
    if input.len().is_multiple_of(4) {
        if input.last() == Some(&61) {
            input.pop();
        }
        if input.last() == Some(&61) {
            input.pop();
        }
    }
    if input.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::new();
    let mut bits = 0u32;
    let mut count = 0;
    for unit in input {
        let value = match unit {
            65..=90 => unit - 65,
            97..=122 => unit - 97 + 26,
            48..=57 => unit - 48 + 52,
            43 => 62,
            47 => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            output.push(((bits >> count) & 255) as u16);
        }
        if output.len() > MAX_EXPANSION / 2 {
            return None;
        }
    }
    Some(JsString::from_units(output))
}
