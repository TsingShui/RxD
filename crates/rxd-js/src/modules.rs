//! Syntax-backed module boundaries and import/export evidence, shared with project navigation.
use crate::{
    fragment::{Contexts, Target},
    lexical::{self, Locations},
    names::RecordBudget,
    strings::JsString,
    structure::Collector,
};
use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::Visit;
use oxc_semantic::{Semantic, SymbolId};
use oxc_span::{GetSpan, Span};
use rxd_core::{Limits, Result, Source, perf::Perf};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy)]
pub enum Listing {
    Modules,
    Imports,
    Exports,
}
#[derive(Clone)]
pub struct ModuleQuery {
    pub listing: Listing,
    pub names: crate::NameQuery,
    pub within: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct Module {
    #[serde(skip)]
    pub at: String,
    #[serde(skip)]
    pub span: SpanInfo,
    pub module_id: Option<JsString>,
    pub format: &'static str,
    pub registry_at: Option<String>,
    pub chunk_ids: Vec<JsString>,
    pub entry: Option<bool>,
    pub imports: usize,
    pub exports: usize,
}
#[derive(Clone, Copy, Serialize)]
pub(crate) struct SpanInfo {
    pub start: u32,
    pub end: u32,
}
impl From<Span> for SpanInfo {
    fn from(s: Span) -> Self {
        Self {
            start: s.start,
            end: s.end,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct Import {
    #[serde(skip)]
    pub span: SpanInfo,
    #[serde(skip)]
    pub module_at: String,
    #[serde(skip)]
    pub context_at: Option<String>,
    pub source: Option<JsString>,
    pub expression: Option<String>,
    pub imported: Option<JsString>,
    pub local: Option<JsString>,
    #[serde(skip)]
    pub binding_at: Option<String>,
    pub syntax: &'static str,
    pub type_only: bool,
}
#[derive(Serialize)]
pub(crate) struct Export {
    #[serde(skip)]
    pub span: SpanInfo,
    #[serde(skip)]
    pub module_at: String,
    #[serde(skip)]
    pub context_at: Option<String>,
    pub exported: JsString,
    pub local: Option<JsString>,
    #[serde(skip)]
    pub local_at: Option<String>,
    pub source: Option<JsString>,
    pub imported: Option<JsString>,
    pub syntax: &'static str,
    pub type_only: bool,
}
pub(crate) struct Reference {
    pub span: SpanInfo,
    pub binding_at: String,
    pub context_at: Option<String>,
    pub parent_at: String,
    pub read: bool,
    pub write: bool,
    pub member_write: bool,
    pub type_reference: Option<bool>,
    pub value_reference: Option<bool>,
}
pub(crate) struct Binding {
    pub at: String,
    pub span: SpanInfo,
}
pub(crate) struct Index {
    pub modules: Vec<Module>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub references: Vec<Reference>,
    pub bindings: Vec<Binding>,
}
#[derive(Clone, Copy)]
struct Factory {
    symbol_module: Option<SymbolId>,
    symbol_exports: Option<SymbolId>,
    symbol_require: Option<SymbolId>,
    index: usize,
    span: Span,
}
struct EntrySite<'a> {
    runtime: SymbolId,
    registry: SymbolId,
    modules: std::ops::Range<usize>,
    dispatch: &'a CallExpression<'a>,
}
#[derive(Default)]
struct EntryPrefixes {
    sites: HashMap<(u32, u32), Option<Span>>,
    activations: HashMap<(u32, u32), bool>,
}

fn symbol(id: &IdentifierReference<'_>, semantic: &Semantic<'_>) -> Option<SymbolId> {
    semantic
        .scoping()
        .get_reference(id.reference_id.get()?)
        .symbol_id()
}
fn id_symbol(expression: &Expression<'_>, semantic: &Semantic<'_>) -> Option<SymbolId> {
    match expression.get_inner_expression() {
        Expression::Identifier(id) => symbol(id, semantic),
        _ => None,
    }
}
fn unwritten(symbol: SymbolId, semantic: &Semantic<'_>) -> bool {
    semantic.scoping().symbol_redeclarations(symbol).is_empty()
        && !semantic
            .scoping()
            .get_resolved_references(symbol)
            .any(|r| r.is_write())
}
fn literal(expression: &Expression<'_>) -> Option<JsString> {
    match expression.get_inner_expression() {
        Expression::StringLiteral(s) => {
            Some(JsString::decoded(s.value.as_str(), s.lone_surrogates))
        }
        Expression::NumericLiteral(n)
            if n.value.is_finite()
                && n.value.fract() == 0.0
                && n.value.abs() <= 9_007_199_254_740_991.0 =>
        {
            Some(JsString::from(n.value.to_string().as_str()))
        }
        _ => None,
    }
}
fn key_name(key: &PropertyKey<'_>, computed: bool) -> Option<JsString> {
    if computed {
        return literal(key.as_expression()?);
    }
    match key {
        PropertyKey::StringLiteral(s) => {
            Some(JsString::decoded(s.value.as_str(), s.lone_surrogates))
        }
        _ => key.static_name().map(|s| JsString::from(s.as_ref())),
    }
}
fn export_name(name: &ModuleExportName<'_>) -> JsString {
    match name {
        ModuleExportName::IdentifierName(id) => JsString::from(id.name.as_str()),
        ModuleExportName::IdentifierReference(id) => JsString::from(id.name.as_str()),
        ModuleExportName::StringLiteral(s) => {
            JsString::decoded(s.value.as_str(), s.lone_surrogates)
        }
    }
}
fn member<'a>(expression: &'a Expression<'a>) -> Option<(&'a Expression<'a>, JsString)> {
    match expression.get_inner_expression() {
        Expression::StaticMemberExpression(m) => {
            Some((&m.object, JsString::from(m.property.name.as_str())))
        }
        Expression::ComputedMemberExpression(m) => Some((&m.object, literal(&m.expression)?)),
        _ => None,
    }
}
fn parameter_symbols(params: &FormalParameters<'_>) -> Vec<Option<SymbolId>> {
    let mut bindings = HashSet::new();
    let mut symbols: Vec<_> = params
        .items
        .iter()
        .map(|p| {
            p.pattern
                .get_binding_identifier()
                .and_then(|id| id.symbol_id.get())
        })
        .collect();
    if symbols
        .iter()
        .flatten()
        .any(|symbol| !bindings.insert(*symbol))
    {
        symbols.fill(None);
    }
    symbols
}
fn coordinate(source: &Source, semantic: &Semantic<'_>, symbol: SymbolId) -> String {
    let s = semantic.scoping().symbol_span(symbol);
    source.entity_id("symbol", s.start, s.end)
}
fn is_global(expression: &Expression<'_>, name: &str, semantic: &Semantic<'_>) -> bool {
    matches!(expression.get_inner_expression(), Expression::Identifier(id) if id.name == name && symbol(id,semantic).is_none())
        && semantic
            .scoping()
            .root_unresolved_references()
            .get(name)
            .is_none_or(|refs| {
                refs.iter()
                    .all(|r| !semantic.scoping().get_reference(*r).is_write())
            })
}

impl Index {
    pub fn build<'a>(
        program: &'a Program<'a>,
        source: &Source,
        collector: &Collector<'_>,
        limits: Limits,
        perf: &mut Perf,
        references: bool,
    ) -> Result<Self> {
        let semantic = lexical::build(program, limits, perf)?;
        let dynamic = lexical::is_dynamic(&semantic);
        let root = source.entity_id("program", 0, source.text.len() as u32);
        let mut index = Self {
            modules: vec![Module {
                at: root.clone(),
                span: Span::new(0, source.text.len() as u32).into(),
                module_id: None,
                format: if program.source_type.is_module() {
                    "esm"
                } else {
                    "script"
                },
                registry_at: None,
                chunk_ids: Vec::new(),
                entry: None,
                imports: 0,
                exports: 0,
            }],
            imports: Vec::new(),
            exports: Vec::new(),
            references: Vec::new(),
            bindings: Vec::new(),
        };
        let mut budget = RecordBudget::new(limits);
        budget.charge(&JsString::from(""))?;
        let mut factories = Vec::new();
        let mut entries = Vec::new();
        let mut registered: HashMap<SymbolId, std::ops::Range<usize>> = HashMap::new();
        let mut registry_reads = HashSet::new();
        if !dynamic {
            // A registry is linked to an actual indexed factory invocation, not just an object of functions.
            for node in semantic.nodes().iter() {
                let AstKind::CallExpression(call) = node.kind() else {
                    continue;
                };
                let callee = call.callee.get_inner_expression();
                let computed = match callee {
                    Expression::ComputedMemberExpression(m) => Some(m.as_ref()),
                    Expression::StaticMemberExpression(m) if m.property.name == "call" => {
                        match m.object.get_inner_expression() {
                            Expression::ComputedMemberExpression(c) => Some(c.as_ref()),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let Some(computed) = computed else { continue };
                let Some(registry) = id_symbol(&computed.object, &semantic) else {
                    continue;
                };
                if !unwritten(registry, &semantic) {
                    continue;
                }
                let decl = semantic.symbol_declaration(registry);
                let table = match decl.kind() {
                    AstKind::VariableDeclarator(d) => d.init.as_ref(),
                    AstKind::FormalParameter(_) => registry_argument(registry, &semantic),
                    _ => None,
                };
                let Some(table) = table else { continue };
                let Some((runtime, direct_selector)) = registry_runtime(call, computed, &semantic)
                else {
                    continue;
                };
                if let Expression::Identifier(id) = computed.object.get_inner_expression() {
                    let key = (id.span.start, id.span.end);
                    if !registry_reads.contains(&key) {
                        budget.charge_bytes(32)?;
                        registry_reads.insert(key);
                    }
                }
                let modules = if let Some(range) = registered.get(&registry) {
                    range.clone()
                } else {
                    let registry_at = crate::fragment::key(decl.kind())
                        .map(|k| k.id(source))
                        .unwrap_or_else(|| root.clone());
                    let start = index.modules.len();
                    add_registry(
                        table,
                        &registry_at,
                        &[],
                        source,
                        &mut index,
                        &mut factories,
                        &mut budget,
                    )?;
                    let range = start..index.modules.len();
                    budget.charge_bytes(64)?;
                    registered.insert(registry, range.clone());
                    range
                };
                if direct_selector {
                    budget.charge_bytes(64)?;
                    entries.push(EntrySite {
                        runtime,
                        registry,
                        modules,
                        dispatch: call,
                    });
                }
            }
            // Webpack async chunks retain a structurally distinct push registration.
            for node in semantic.nodes().iter() {
                let AstKind::CallExpression(call) = node.kind() else {
                    continue;
                };
                let Some((receiver, name)) = member(&call.callee) else {
                    continue;
                };
                if !name.equals_str("push")
                    || call.arguments.len() != 1
                    || !chunk_receiver(receiver, &semantic)
                {
                    continue;
                }
                let Some(Expression::ArrayExpression(payload)) = call.arguments[0]
                    .as_expression()
                    .map(Expression::get_inner_expression)
                else {
                    continue;
                };
                let Some(ArrayExpressionElement::ArrayExpression(ids)) = payload.elements.first()
                else {
                    continue;
                };
                let Some(table) = payload
                    .elements
                    .get(1)
                    .and_then(ArrayExpressionElement::as_expression)
                else {
                    continue;
                };
                let Some(ids) = ids
                    .elements
                    .iter()
                    .map(|e| e.as_expression().and_then(literal))
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                add_registry(
                    table,
                    &crate::refs::context_at(&semantic, node.id(), source)
                        .unwrap_or_else(|| root.clone()),
                    &ids,
                    source,
                    &mut index,
                    &mut factories,
                    &mut budget,
                )?;
            }
        }
        for factory in &mut factories {
            for role in [
                &mut factory.symbol_module,
                &mut factory.symbol_exports,
                &mut factory.symbol_require,
            ] {
                *role = role.filter(|symbol| unwritten(*symbol, &semantic));
            }
        }
        let mut visitor = Visitor {
            source,
            semantic: &semantic,
            index: &mut index,
            factories: &factories,
            contexts: Contexts::new(Target::Matches),
            budget: &mut budget,
            error: None,
            dynamic,
            root: &root,
        };
        visitor.visit_program(program);
        if let Some(error) = visitor.error {
            return Err(error);
        }
        let dependencies_closed = closed_entry_dependencies(program);
        let mut prefixes = EntryPrefixes::default();
        let mut stable_registries: HashMap<SymbolId, Option<(Span, u32)>> = HashMap::new();
        for entry in entries {
            if !dependencies_closed {
                break;
            }
            let scope = if let Some(scope) = stable_registries.get(&entry.registry) {
                *scope
            } else {
                budget.charge_bytes(64)?;
                let scope = entry_registry(entry.registry, &semantic, &registry_reads);
                stable_registries.insert(entry.registry, scope);
                scope
            };
            let Some((frame, ready)) = scope else {
                continue;
            };
            if !entry_dispatch(entry.dispatch, &semantic, &mut prefixes, &mut budget)? {
                continue;
            }
            for reference in semantic.scoping().get_resolved_references(entry.runtime) {
                let AstKind::CallExpression(call) =
                    semantic.nodes().parent_kind(reference.node_id())
                else {
                    continue;
                };
                if call.callee.span() != semantic.reference_span(reference)
                    || call.arguments.len() != 1
                    || !runtime_ready(entry.runtime, call.span, &semantic)
                    || call.span.start < ready
                    || prefixes.call(call, &semantic, &mut budget)? != Some(frame)
                    || !entry_activation(call, &semantic, &mut prefixes, &mut budget)?
                    || factories.iter().any(|f| contains(f.span, call.span))
                {
                    continue;
                }
                let Some(id) = call.arguments[0].as_expression().and_then(literal) else {
                    continue;
                };
                for i in entry.modules.clone() {
                    let m = &mut index.modules[i];
                    if m.module_id.as_ref() == Some(&id) {
                        m.entry = Some(true);
                    }
                }
            }
        }
        for m in &mut index.modules {
            m.imports = index.imports.iter().filter(|i| i.module_at == m.at).count();
            m.exports = index.exports.iter().filter(|e| e.module_at == m.at).count();
        }
        if references && !dynamic {
            let locations = Locations::new(collector);
            for binding in semantic.scoping().symbol_ids() {
                budget.charge(&JsString::from(semantic.scoping().symbol_name(binding)))?;
                index.bindings.push(Binding {
                    at: coordinate(source, &semantic, binding),
                    span: semantic.scoping().symbol_span(binding).into(),
                });
                for reference in semantic.scoping().get_resolved_references(binding) {
                    let span = semantic.reference_span(reference);
                    budget.charge(&JsString::from(semantic.scoping().symbol_name(binding)))?;
                    index.references.push(Reference {
                        span: span.into(),
                        binding_at: coordinate(source, &semantic, binding),
                        context_at: crate::refs::context_at(&semantic, reference.node_id(), source),
                        parent_at: locations.get(span).entity_id,
                        read: reference.is_read(),
                        write: reference.is_write(),
                        member_write: reference.flags().is_member_write_target(),
                        type_reference: program
                            .source_type
                            .is_typescript()
                            .then(|| reference.is_type()),
                        value_reference: program
                            .source_type
                            .is_typescript()
                            .then(|| reference.is_value()),
                    });
                }
            }
        }
        index.references.sort_by_key(|r| (r.span.start, r.span.end));
        index.imports.sort_by_key(|i| (i.span.start, i.span.end));
        index.exports.sort_by_key(|e| (e.span.start, e.span.end));
        Ok(index)
    }
}
fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}
fn registry_argument<'a>(symbol: SymbolId, semantic: &Semantic<'a>) -> Option<&'a Expression<'a>> {
    let decl = semantic.symbol_declaration(symbol);
    let function = semantic
        .nodes()
        .ancestors(decl.id())
        .find_map(|n| match n.kind() {
            AstKind::Function(f) => Some(f),
            _ => None,
        })?;
    let index = parameter_symbols(&function.params)
        .iter()
        .position(|p| *p == Some(symbol))?;
    semantic
        .nodes()
        .ancestors(function.node_id.get())
        .find_map(|n| match n.kind() {
            AstKind::CallExpression(c)
                if c.callee.get_inner_expression().span() == function.span =>
            {
                if c.arguments
                    .iter()
                    .take(index + 1)
                    .any(|a| matches!(a, Argument::SpreadElement(_)))
                {
                    return None;
                }
                c.arguments.get(index)?.as_expression()
            }
            _ => None,
        })
}
fn registry_runtime(
    call: &CallExpression<'_>,
    indexed: &ComputedMemberExpression<'_>,
    semantic: &Semantic<'_>,
) -> Option<(SymbolId, bool)> {
    let extra = usize::from(matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(_)
    ));
    if call.optional || indexed.optional || call.arguments.len() != 3 + extra {
        return None;
    }
    let module = id_symbol(call.arguments[extra].as_expression()?, semantic)?;
    let (object, property) = member(call.arguments[extra + 1].as_expression()?)?;
    if !property.equals_str("exports") || id_symbol(object, semantic) != Some(module) {
        return None;
    }
    let runtime = id_symbol(call.arguments[extra + 2].as_expression()?, semantic)?;
    if !unwritten(runtime, semantic) {
        return None;
    }
    let owner = semantic.nodes().ancestors(call.node_id.get()).find(|n| {
        matches!(
            n.kind(),
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
        )
    })?;
    let params = match owner.kind() {
        AstKind::Function(f) => &f.params,
        AstKind::ArrowFunctionExpression(f) => &f.params,
        _ => return None,
    };
    let selector = params
        .items
        .first()?
        .pattern
        .get_binding_identifier()?
        .symbol_id
        .get()?;
    if id_symbol(&indexed.expression, semantic) != Some(selector) {
        return None;
    }
    let declaration = semantic.symbol_declaration(runtime);
    let span = match declaration.kind() {
        AstKind::Function(f) => f.span,
        AstKind::VariableDeclarator(d) => d.init.as_ref()?.get_inner_expression().span(),
        _ => return None,
    };
    (span == owner.span()).then_some((
        runtime,
        unwritten(selector, semantic)
            && parameter_symbols(params).first().copied().flatten() == Some(selector),
    ))
}
fn pure_entry_value(expression: &Expression<'_>, depth: usize) -> bool {
    if depth >= 32 {
        return false;
    }
    match expression.get_inner_expression() {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::FunctionExpression(_)
        | Expression::ArrowFunctionExpression(_) => true,
        Expression::TemplateLiteral(t) => t.expressions.is_empty(),
        Expression::ArrayExpression(a) => a.elements.iter().all(|e| {
            matches!(e, ArrayExpressionElement::Elision(_))
                || e.as_expression()
                    .is_some_and(|e| pure_entry_value(e, depth + 1))
        }),
        Expression::ObjectExpression(o) => o.properties.iter().all(|p| {
            let ObjectPropertyKind::ObjectProperty(p) = p else {
                return false;
            };
            p.kind == PropertyKind::Init
                && key_name(&p.key, p.computed).is_some_and(|name| !name.equals_str("__proto__"))
                && pure_entry_value(&p.value, depth + 1)
        }),
        _ => false,
    }
}
fn pure_entry_statement(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::FunctionDeclaration(_) | Statement::EmptyStatement(_) => true,
        Statement::ImportDeclaration(d) => d.import_kind == ImportOrExportKind::Type,
        Statement::ExportAllDeclaration(d) => d.export_kind == ImportOrExportKind::Type,
        Statement::ExportNamedDeclaration(d) => d.export_kind == ImportOrExportKind::Type,
        Statement::ExportFromDeclaration(d) => d.export_kind == ImportOrExportKind::Type,
        Statement::VariableDeclaration(d) => {
            matches!(
                d.kind,
                VariableDeclarationKind::Var
                    | VariableDeclarationKind::Let
                    | VariableDeclarationKind::Const
            ) && d.declarations.iter().all(|d| {
                d.id.get_binding_identifier().is_some()
                    && d.init.as_ref().is_none_or(|e| pure_entry_value(e, 0))
            })
        }
        _ => false,
    }
}
impl EntryPrefixes {
    fn activation(
        &mut self,
        invocation: &CallExpression<'_>,
        owner: Span,
        params: &FormalParameters<'_>,
        semantic: &Semantic<'_>,
        budget: &mut RecordBudget,
    ) -> Result<bool> {
        let key = (owner.start, owner.end);
        if let Some(valid) = self.activations.get(&key) {
            return Ok(*valid);
        }
        let valid = simple_entry_parameters(params)
            && !invocation.optional
            && invocation.callee.get_inner_expression().span() == owner
            && invocation
                .arguments
                .iter()
                .all(|a| a.as_expression().is_some_and(|e| pure_entry_value(e, 0)))
            && self.call(invocation, semantic, budget)?.is_some();
        budget.charge_bytes(64)?;
        self.activations.insert(key, valid);
        Ok(valid)
    }
    fn call(
        &mut self,
        call: &CallExpression<'_>,
        semantic: &Semantic<'_>,
        budget: &mut RecordBudget,
    ) -> Result<Option<Span>> {
        let mut node = semantic.nodes().parent_node(call.node_id.get());
        while matches!(node.kind(), AstKind::ParenthesizedExpression(_)) {
            node = semantic.nodes().parent_node(node.id());
        }
        let statement = match node.kind() {
            AstKind::ExpressionStatement(e)
                if e.expression.get_inner_expression().span() == call.span =>
            {
                node
            }
            AstKind::ReturnStatement(r)
                if r.argument
                    .as_ref()
                    .is_some_and(|e| e.get_inner_expression().span() == call.span) =>
            {
                node
            }
            AstKind::VariableDeclarator(d)
                if d.id.get_binding_identifier().is_some()
                    && d.init
                        .as_ref()
                        .is_some_and(|e| e.get_inner_expression().span() == call.span) =>
            {
                let parent = semantic.nodes().parent_node(node.id());
                if !matches!(parent.kind(),AstKind::VariableDeclaration(v) if v.declarations.len()==1 && matches!(v.kind,VariableDeclarationKind::Var | VariableDeclarationKind::Let | VariableDeclarationKind::Const))
                {
                    return Ok(None);
                }
                parent
            }
            _ => return Ok(None),
        };
        let frame = semantic.nodes().parent_node(statement.id());
        let body = match frame.kind() {
            AstKind::Program(p) => p.body.as_slice(),
            AstKind::FunctionBody(f) => f.statements.as_slice(),
            _ => return Ok(None),
        };
        let span = frame.span();
        let key = (span.start, span.end);
        let first = if let Some(first) = self.sites.get(&key) {
            *first
        } else {
            budget.charge_bytes(64)?;
            let first = body
                .iter()
                .find(|s| !pure_entry_statement(s))
                .map(GetSpan::span);
            self.sites.insert(key, first);
            first
        };
        Ok((first == Some(statement.span())).then_some(span))
    }
}
fn simple_entry_parameters(params: &FormalParameters<'_>) -> bool {
    params.rest.is_none()
        && params
            .items
            .iter()
            .all(|p| p.initializer.is_none() && p.pattern.get_binding_identifier().is_some())
}
fn entry_registry(
    registry: SymbolId,
    semantic: &Semantic<'_>,
    reads: &HashSet<(u32, u32)>,
) -> Option<(Span, u32)> {
    if !unwritten(registry, semantic)
        || !semantic
            .scoping()
            .get_resolved_references(registry)
            .all(|r| {
                let s = semantic.reference_span(r);
                reads.contains(&(s.start, s.end))
            })
    {
        return None;
    }
    let declaration = semantic.symbol_declaration(registry);
    match declaration.kind() {
        AstKind::VariableDeclarator(d) => {
            let statement = semantic.nodes().parent_node(declaration.id());
            let AstKind::VariableDeclaration(variable) = statement.kind() else {
                return None;
            };
            let frame = semantic.nodes().parent_node(statement.id());
            if !matches!(frame.kind(), AstKind::Program(_) | AstKind::FunctionBody(_))
                || (matches!(frame.kind(), AstKind::Program(_))
                    && semantic.source_type().is_script()
                    && variable.kind == VariableDeclarationKind::Var)
            {
                return None;
            }
            Some((frame.span(), d.span.end))
        }
        AstKind::FormalParameter(_) => {
            let f = semantic.nodes().ancestors(declaration.id()).find_map(|n| {
                if let AstKind::Function(f) = n.kind() {
                    Some(f)
                } else {
                    None
                }
            })?;
            if f.r#async || f.generator || !simple_entry_parameters(&f.params) {
                return None;
            }
            f.body.as_ref().map(|body| (body.span, body.span.start))
        }
        _ => None,
    }
}
fn entry_dispatch(
    call: &CallExpression<'_>,
    semantic: &Semantic<'_>,
    prefixes: &mut EntryPrefixes,
    budget: &mut RecordBudget,
) -> Result<bool> {
    if !matches!(
        call.callee.get_inner_expression(),
        Expression::ComputedMemberExpression(_)
    ) {
        return Ok(false);
    }
    let Some(owner) = semantic.nodes().ancestors(call.node_id.get()).find(|n| {
        matches!(
            n.kind(),
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
        )
    }) else {
        return Ok(false);
    };
    let valid = match owner.kind() {
        AstKind::Function(f) => !f.r#async && !f.generator && simple_entry_parameters(&f.params),
        AstKind::ArrowFunctionExpression(f) => !f.r#async && simple_entry_parameters(&f.params),
        _ => false,
    };
    if !valid {
        return Ok(false);
    }
    let Some(frame) = prefixes.call(call, semantic, budget)? else {
        return Ok(false);
    };
    let Some(module) = call
        .arguments
        .first()
        .and_then(Argument::as_expression)
        .and_then(|e| id_symbol(e, semantic))
    else {
        return Ok(false);
    };
    if !unwritten(module, semantic) {
        return Ok(false);
    }
    let declaration = semantic.symbol_declaration(module);
    let AstKind::VariableDeclarator(d) = declaration.kind() else {
        return Ok(false);
    };
    let statement = semantic.nodes().parent_node(declaration.id());
    let parent = semantic.nodes().parent_node(statement.id());
    let Some(Expression::ObjectExpression(object)) =
        d.init.as_ref().map(Expression::get_inner_expression)
    else {
        return Ok(false);
    };
    Ok(parent.span()==frame && d.span.end<=call.span.start && object.properties.iter().any(|p|matches!(p,ObjectPropertyKind::ObjectProperty(p) if p.kind==PropertyKind::Init && key_name(&p.key,p.computed).is_some_and(|name|name.equals_str("exports")))))
}
fn runtime_ready(runtime: SymbolId, call: Span, semantic: &Semantic<'_>) -> bool {
    let declaration = semantic.symbol_declaration(runtime);
    match declaration.kind() {
        AstKind::Function(function) => lexical::hoisted_function(function, semantic),
        AstKind::VariableDeclarator(d) if call.start >= d.span.end => {
            let statement = semantic.nodes().parent_node(declaration.id());
            matches!(
                semantic.nodes().parent_kind(statement.id()),
                AstKind::Program(_) | AstKind::FunctionBody(_)
            )
        }
        _ => false,
    }
}
fn closed_entry_dependencies(program: &Program<'_>) -> bool {
    program.body.iter().all(|statement| match statement {
        Statement::ImportDeclaration(d) => d.import_kind == ImportOrExportKind::Type,
        Statement::ExportAllDeclaration(d) => d.export_kind == ImportOrExportKind::Type,
        Statement::ExportFromDeclaration(d) => d.export_kind == ImportOrExportKind::Type,
        _ => true,
    })
}
fn entry_activation(
    call: &CallExpression<'_>,
    semantic: &Semantic<'_>,
    prefixes: &mut EntryPrefixes,
    budget: &mut RecordBudget,
) -> Result<bool> {
    for node in semantic.nodes().ancestors(call.node_id.get()) {
        if matches!(
            node.kind(),
            AstKind::IfStatement(_)
                | AstKind::ConditionalExpression(_)
                | AstKind::LogicalExpression(_)
                | AstKind::SwitchStatement(_)
                | AstKind::ForStatement(_)
                | AstKind::ForInStatement(_)
                | AstKind::ForOfStatement(_)
                | AstKind::WhileStatement(_)
                | AstKind::DoWhileStatement(_)
                | AstKind::TryStatement(_)
        ) {
            return Ok(false);
        }
        let (span, params) = match node.kind() {
            AstKind::Function(f) if !f.r#async && !f.generator => (f.span, &f.params),
            AstKind::ArrowFunctionExpression(f) if !f.r#async => (f.span, &f.params),
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_) => return Ok(false),
            _ => continue,
        };
        let Some(parent) = semantic
            .nodes()
            .ancestors(node.id())
            .find(|n| !matches!(n.kind(), AstKind::ParenthesizedExpression(_)))
        else {
            return Ok(false);
        };
        let AstKind::CallExpression(invocation) = parent.kind() else {
            return Ok(false);
        };
        if !prefixes.activation(invocation, span, params, semantic, budget)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn chunk_receiver(expression: &Expression<'_>, semantic: &Semantic<'_>) -> bool {
    let valid = |object: &Expression<'_>, name: &JsString| {
        ["self", "window", "globalThis"]
            .iter()
            .any(|global| is_global(object, global, semantic))
            && name
                .to_utf8()
                .is_some_and(|name| name.starts_with("webpackChunk") || name == "webpackJsonp")
    };
    match expression.get_inner_expression() {
        Expression::AssignmentExpression(a)
            if a.operator == oxc_syntax::operator::AssignmentOperator::Assign =>
        {
            let (object, name) = match &a.left {
                AssignmentTarget::StaticMemberExpression(m) => {
                    (&m.object, JsString::from(m.property.name.as_str()))
                }
                AssignmentTarget::ComputedMemberExpression(m) => {
                    let Some(name) = literal(&m.expression) else {
                        return false;
                    };
                    (&m.object, name)
                }
                _ => return false,
            };
            let Expression::LogicalExpression(right) = a.right.get_inner_expression() else {
                return false;
            };
            valid(object,&name) && right.operator == oxc_syntax::operator::LogicalOperator::Or &&
            member(&right.left).is_some_and(|(o,n)| n==name && matches!((object.get_inner_expression(),o.get_inner_expression()),(Expression::Identifier(a),Expression::Identifier(b)) if a.name==b.name)) &&
            matches!(right.right.get_inner_expression(),Expression::ArrayExpression(a) if a.elements.is_empty())
        }
        _ => member(expression).is_some_and(|(object, name)| valid(object, &name)),
    }
}
fn add_registry(
    table: &Expression<'_>,
    registry: &str,
    chunks: &[JsString],
    source: &Source,
    index: &mut Index,
    factories: &mut Vec<Factory>,
    budget: &mut RecordBudget,
) -> Result<()> {
    if let Expression::ObjectExpression(object) = table.get_inner_expression()
        && object.properties.iter().any(|p| match p {
            ObjectPropertyKind::ObjectProperty(p) => {
                p.kind != PropertyKind::Init
                    || key_name(&p.key, p.computed)
                        .is_none_or(|name| !p.method && !p.computed && name.equals_str("__proto__"))
            }
            _ => true,
        })
    {
        return Ok(());
    }
    if matches!(table.get_inner_expression(),Expression::ArrayExpression(a) if a.elements.iter().any(|e|matches!(e,ArrayExpressionElement::SpreadElement(_))))
    {
        return Ok(());
    }
    let entries: Vec<_> = match table.get_inner_expression() {
        Expression::ObjectExpression(o) => o
            .properties
            .iter()
            .filter_map(|p| match p {
                ObjectPropertyKind::ObjectProperty(p) if p.kind == PropertyKind::Init => Some((
                    key_name(&p.key, p.computed)?,
                    &p.value,
                    p.method.then_some(p.span),
                )),
                _ => None,
            })
            .collect(),
        Expression::ArrayExpression(a) => a
            .elements
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                Some((
                    JsString::from(i.to_string().as_str()),
                    e.as_expression()?,
                    None,
                ))
            })
            .collect(),
        _ => return Ok(()),
    };
    let last: HashMap<_, _> = entries
        .iter()
        .enumerate()
        .map(|(i, (id, _, _))| (id.units(), i))
        .collect();
    for (position, (id, expression, method_span)) in entries.iter().enumerate() {
        if last.get(id.units()) != Some(&position) {
            continue;
        }
        let (kind, span, params) = match expression.get_inner_expression() {
            Expression::FunctionExpression(f) => ("function_expression", f.span, &f.params),
            Expression::ArrowFunctionExpression(f) => ("arrow_function", f.span, &f.params),
            _ => continue,
        };
        let (kind, span) = method_span.map_or((kind, span), |s| ("object_method", s));
        if factories.iter().any(|f| f.span == span) {
            continue;
        }
        budget.charge(id)?;
        budget.charge_bytes(registry.len().saturating_add(chunks.iter().fold(
            0usize,
            |bytes, id| {
                bytes
                    .saturating_add(std::mem::size_of::<JsString>())
                    .saturating_add(id.len_utf16().saturating_mul(2))
            },
        )))?;
        let params = parameter_symbols(params);
        factories.push(Factory {
            symbol_module: params.first().copied().flatten(),
            symbol_exports: params.get(1).copied().flatten(),
            symbol_require: params.get(2).copied().flatten(),
            index: index.modules.len(),
            span,
        });
        index.modules.push(Module {
            at: source.entity_id(kind, span.start, span.end),
            span: span.into(),
            module_id: Some(id.clone()),
            format: "webpack",
            registry_at: Some(registry.to_owned()),
            chunk_ids: chunks.to_vec(),
            entry: None,
            imports: 0,
            exports: 0,
        });
    }
    Ok(())
}
enum ExportTarget<'a> {
    Local(Option<SymbolId>),
    Forward {
        source: &'a JsString,
        imported: JsString,
    },
}
struct Visitor<'a, 's> {
    source: &'s Source,
    semantic: &'a Semantic<'s>,
    index: &'a mut Index,
    factories: &'a [Factory],
    contexts: Contexts,
    budget: &'a mut RecordBudget,
    error: Option<rxd_core::Error>,
    dynamic: bool,
    root: &'a str,
}
impl Visitor<'_, '_> {
    fn factory(&self, span: Span) -> Option<Factory> {
        self.factories
            .iter()
            .filter(|f| contains(f.span, span))
            .min_by_key(|f| f.span.size())
            .copied()
    }
    fn module(&self, span: Span) -> String {
        self.factory(span).map_or_else(
            || self.root.to_owned(),
            |f| self.index.modules[f.index].at.clone(),
        )
    }
    fn import(
        &mut self,
        span: Span,
        source: Option<&JsString>,
        imported: Option<JsString>,
        local: Option<&BindingIdentifier<'_>>,
        syntax: &'static str,
        type_only: bool,
    ) {
        let dynamic = JsString::from("<dynamic>");
        let label = imported.as_ref().or(source).unwrap_or(&dynamic);
        let extra = source
            .map_or(0, |s| s.len_utf16().saturating_mul(2))
            .saturating_add(local.map_or(0, |id| id.name.len().saturating_mul(2)));
        if let Err(e) = self
            .budget
            .charge(label)
            .and_then(|_| self.budget.charge_bytes(extra))
        {
            self.error = Some(e);
            return;
        }
        self.index.imports.push(Import {
            span: span.into(),
            module_at: self.module(span),
            context_at: self.contexts.current_id(self.source),
            source: source.cloned(),
            expression: None,
            imported,
            local: local.map(|id| JsString::from(id.name.as_str())),
            binding_at: local
                .and_then(|id| id.symbol_id.get())
                .map(|s| coordinate(self.source, self.semantic, s)),
            syntax,
            type_only,
        });
    }
    fn import_expression(&mut self, expression: Option<&Expression<'_>>) {
        if self.error.is_some() {
            return;
        }
        if let Some(row) = self.index.imports.last_mut()
            && row.source.is_none()
            && let Some(expression) = expression
        {
            let span = expression.span();
            if let Err(error) = self.budget.charge_bytes(span.size() as usize) {
                self.error = Some(error);
                return;
            }
            row.expression =
                Some(self.source.text[span.start as usize..span.end as usize].to_owned());
        }
    }
    fn export(
        &mut self,
        span: Span,
        exported: JsString,
        target: ExportTarget<'_>,
        syntax: &'static str,
        type_only: bool,
    ) {
        let extra = match &target {
            ExportTarget::Local(symbol) => symbol.map_or(0, |s| {
                self.semantic
                    .scoping()
                    .symbol_name(s)
                    .len()
                    .saturating_mul(2)
            }),
            ExportTarget::Forward { source, imported } => source
                .len_utf16()
                .saturating_add(imported.len_utf16())
                .saturating_mul(2),
        };
        if let Err(e) = self
            .budget
            .charge(&exported)
            .and_then(|_| self.budget.charge_bytes(extra))
        {
            self.error = Some(e);
            return;
        }
        let (local, local_at, source, imported) = match target {
            ExportTarget::Local(symbol) => (
                symbol.map(|s| JsString::from(self.semantic.scoping().symbol_name(s))),
                symbol.map(|s| coordinate(self.source, self.semantic, s)),
                None,
                None,
            ),
            ExportTarget::Forward { source, imported } => {
                (None, None, Some(source.clone()), Some(imported))
            }
        };
        self.index.exports.push(Export {
            span: span.into(),
            module_at: self.module(span),
            context_at: self.contexts.current_id(self.source),
            exported,
            local,
            local_at,
            source,
            imported,
            syntax,
            type_only,
        });
    }
    fn local_export(
        &mut self,
        span: Span,
        exported: JsString,
        reference: Option<SymbolId>,
        syntax: &'static str,
        type_only: bool,
    ) {
        self.export(
            span,
            exported,
            ExportTarget::Local(reference),
            syntax,
            type_only,
        );
    }
}
impl<'s> Visit<'s> for Visitor<'_, 's> {
    fn enter_node(&mut self, node: AstKind<'s>) {
        self.contexts.enter(node, 0);
        if self.error.is_some() {
            return;
        }
        match node {
            AstKind::ImportDeclaration(d) => {
                let source = JsString::decoded(d.source.value.as_str(), d.source.lone_surrogates);
                if d.specifiers.as_ref().is_none_or(|s| s.is_empty()) {
                    self.import(
                        d.span,
                        Some(&source),
                        None,
                        None,
                        "esm",
                        d.import_kind == ImportOrExportKind::Type,
                    );
                }
                if let Some(specifiers) = &d.specifiers {
                    for s in specifiers {
                        let (name, local, typed) = match s {
                            ImportDeclarationSpecifier::ImportSpecifier(s) => (
                                export_name(&s.imported),
                                &s.local,
                                s.import_kind == ImportOrExportKind::Type,
                            ),
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                (JsString::from("default"), &s.local, false)
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                (JsString::from("*"), &s.local, false)
                            }
                        };
                        self.import(
                            s.span(),
                            Some(&source),
                            Some(name),
                            Some(local),
                            "esm",
                            typed || d.import_kind == ImportOrExportKind::Type,
                        );
                    }
                }
            }
            AstKind::ExportDeclaration(d) => {
                let span = d.declaration.span();
                let symbols: Vec<_> = self
                    .semantic
                    .scoping()
                    .symbol_ids()
                    .filter(|s| {
                        contains(span, self.semantic.scoping().symbol_span(*s))
                            && self.semantic.scoping().symbol_scope_id(*s)
                                == self.semantic.nodes().get_node(d.node_id.get()).scope_id()
                    })
                    .collect();
                for symbol in symbols {
                    self.local_export(
                        span,
                        JsString::from(self.semantic.scoping().symbol_name(symbol)),
                        Some(symbol),
                        "esm",
                        !self.semantic.scoping().symbol_flags(symbol).is_value(),
                    );
                }
            }
            AstKind::ExportNamedDeclaration(d) => {
                for s in &d.specifiers {
                    let reference = match &s.local {
                        ModuleExportName::IdentifierReference(id) => symbol(id, self.semantic),
                        _ => None,
                    };
                    self.local_export(
                        s.span,
                        export_name(&s.exported),
                        reference,
                        "esm",
                        d.export_kind == ImportOrExportKind::Type
                            || s.export_kind == ImportOrExportKind::Type,
                    );
                }
            }
            AstKind::ExportFromDeclaration(d) => {
                let source = JsString::decoded(d.source.value.as_str(), d.source.lone_surrogates);
                for s in &d.specifiers {
                    self.export(
                        s.span,
                        export_name(&s.exported),
                        ExportTarget::Forward {
                            source: &source,
                            imported: export_name(&s.local),
                        },
                        "reexport",
                        d.export_kind == ImportOrExportKind::Type
                            || s.export_kind == ImportOrExportKind::Type,
                    );
                }
            }
            AstKind::ExportAllDeclaration(d) => {
                let source = JsString::decoded(d.source.value.as_str(), d.source.lone_surrogates);
                self.export(
                    d.span,
                    d.exported
                        .as_ref()
                        .map_or_else(|| JsString::from("*"), export_name),
                    ExportTarget::Forward {
                        source: &source,
                        imported: JsString::from("*"),
                    },
                    "star",
                    d.export_kind == ImportOrExportKind::Type,
                );
            }
            AstKind::ExportDefaultDeclaration(d) => {
                let binding = match &d.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                        f.id.as_ref().and_then(|id| id.symbol_id.get())
                    }
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                        c.id.as_ref().and_then(|id| id.symbol_id.get())
                    }
                    // An expression snapshots a value, unlike a live export specifier.
                    ExportDefaultDeclarationKind::Identifier(_) => None,
                    _ => None,
                };
                self.local_export(d.span, JsString::from("default"), binding, "esm", false);
            }
            AstKind::ImportExpression(i) => {
                self.import(
                    i.span,
                    literal(&i.source).as_ref(),
                    Some(JsString::from("*")),
                    None,
                    "dynamic_import",
                    false,
                );
                self.import_expression(Some(&i.source));
            }
            AstKind::CallExpression(call) if !self.dynamic => {
                let factory = self.factory(call.span);
                let bundled = factory
                    .and_then(|f| f.symbol_require)
                    .is_some_and(|s| id_symbol(&call.callee, self.semantic) == Some(s));
                if bundled || is_global(&call.callee, "require", self.semantic) {
                    if call.arguments.len() != 1 {
                        return;
                    }
                    let expression = call.arguments[0].as_expression();
                    let local = match self.semantic.nodes().parent_kind(call.node_id.get()) {
                        AstKind::VariableDeclarator(d) => d.id.get_binding_identifier(),
                        _ => None,
                    };
                    self.import(
                        call.span,
                        expression
                            .and_then(|e| {
                                if bundled
                                    || matches!(
                                        e.get_inner_expression(),
                                        Expression::StringLiteral(_)
                                    )
                                {
                                    literal(e)
                                } else {
                                    None
                                }
                            })
                            .as_ref(),
                        Some(JsString::from("*")),
                        local,
                        if bundled {
                            "webpack_require"
                        } else {
                            "commonjs"
                        },
                        false,
                    );
                    self.import_expression(expression);
                } else if let Some((object, name)) = member(&call.callee)
                    && name.equals_str("d")
                    && factory
                        .and_then(|f| f.symbol_require)
                        .is_some_and(|s| id_symbol(object, self.semantic) == Some(s))
                    && call.arguments.len() == 2
                    && factory
                        .and_then(|f| f.symbol_exports)
                        .is_some_and(|exports| {
                            call.arguments[0]
                                .as_expression()
                                .and_then(|e| id_symbol(e, self.semantic))
                                == Some(exports)
                        })
                {
                    let Some(Expression::ObjectExpression(exports)) = call.arguments[1]
                        .as_expression()
                        .map(Expression::get_inner_expression)
                    else {
                        return;
                    };
                    for p in &exports.properties {
                        let ObjectPropertyKind::ObjectProperty(p) = p else {
                            continue;
                        };
                        let Some(name) = key_name(&p.key, p.computed) else {
                            continue;
                        };
                        self.local_export(p.span, name, None, "webpack_getter", false);
                    }
                }
            }
            AstKind::AssignmentExpression(a) if !self.dynamic => {
                let (object, name) = match &a.left {
                    AssignmentTarget::StaticMemberExpression(m) => {
                        (&m.object, JsString::from(m.property.name.as_str()))
                    }
                    AssignmentTarget::ComputedMemberExpression(m) => {
                        let Some(name) = literal(&m.expression) else {
                            return;
                        };
                        (&m.object, name)
                    }
                    _ => return,
                };
                let factory = self.factory(a.span);
                let exports = factory.and_then(|f| f.symbol_exports);
                let module = factory.and_then(|f| f.symbol_module);
                let is_exports = exports
                    .is_some_and(|s| id_symbol(object, self.semantic) == Some(s))
                    || is_global(object, "exports", self.semantic)
                    || member(object).is_some_and(|(o, n)| {
                        n.equals_str("exports")
                            && (module.is_some_and(|s| id_symbol(o, self.semantic) == Some(s))
                                || is_global(o, "module", self.semantic))
                    });
                let whole = name.equals_str("exports")
                    && (module.is_some_and(|s| id_symbol(object, self.semantic) == Some(s))
                        || is_global(object, "module", self.semantic));
                let syntax = if factory.is_some() {
                    "webpack"
                } else {
                    "commonjs"
                };
                if whole
                    && let Expression::ObjectExpression(object) = a.right.get_inner_expression()
                {
                    for property in &object.properties {
                        let ObjectPropertyKind::ObjectProperty(property) = property else {
                            continue;
                        };
                        let Some(name) = key_name(&property.key, property.computed) else {
                            continue;
                        };
                        self.local_export(
                            property.span,
                            name,
                            id_symbol(&property.value, self.semantic),
                            syntax,
                            false,
                        );
                    }
                }
                if is_exports || whole {
                    self.local_export(
                        a.span,
                        if whole {
                            JsString::from("default")
                        } else {
                            name
                        },
                        id_symbol(&a.right, self.semantic),
                        syntax,
                        false,
                    );
                    if factory.is_none() {
                        let format = &mut self.index.modules[0].format;
                        if *format == "script" {
                            *format = "commonjs";
                        } else if *format == "esm" {
                            *format = "mixed";
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn leave_node(&mut self, _: AstKind<'s>) {
        self.contexts.leave();
    }
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Row {
    Module(Module),
    Import(Import),
    Export(Export),
}
impl Row {
    pub fn envelope(&self) -> crate::envelope::Body<&Self> {
        let (kind, at, parent_at, context_at, span) = match self {
            Self::Module(m) => ("module", Some(m.at.as_str()), None, None, m.span),
            Self::Import(i) => (
                "import",
                i.binding_at.as_deref(),
                Some(i.module_at.as_str()),
                i.context_at.as_deref(),
                i.span,
            ),
            Self::Export(e) => (
                "export",
                e.local_at.as_deref(),
                Some(e.module_at.as_str()),
                e.context_at.as_deref(),
                e.span,
            ),
        };
        crate::envelope::body(
            kind,
            rxd_core::envelope::RecordMeta {
                at,
                parent_at,
                context_at,
                start: Some(span.start as usize),
                end: Some(span.end as usize),
                ..Default::default()
            },
            self,
        )
    }
}
pub(crate) fn run(
    program: &Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    query: &ModuleQuery,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<Row>> {
    let region = query
        .within
        .as_deref()
        .map(|at| collector.by_id(at))
        .transpose()?
        .map(|i| {
            Span::new(
                collector.entities[i].entity.start,
                collector.entities[i].entity.end,
            )
        });
    let index = Index::build(program, source, collector, limits, perf, false)?;
    let names = query.names.compile();
    let in_region = |s: SpanInfo| region.is_none_or(|r| contains(r, Span::new(s.start, s.end)));
    Ok(match query.listing {
        Listing::Modules => index
            .modules
            .into_iter()
            .filter(|m| {
                in_region(m.span)
                    && m.module_id.as_ref().map_or(
                        query.names.name.is_none() && query.names.filter.is_none(),
                        |id| names.matches(id),
                    )
            })
            .map(Row::Module)
            .collect(),
        Listing::Imports => {
            let dynamic = JsString::from("<dynamic>");
            index
                .imports
                .into_iter()
                .filter(|i| {
                    in_region(i.span)
                        && names.matches(
                            i.imported
                                .as_ref()
                                .or(i.source.as_ref())
                                .unwrap_or(&dynamic),
                        )
                })
                .map(Row::Import)
                .collect()
        }
        Listing::Exports => index
            .exports
            .into_iter()
            .filter(|e| in_region(e.span) && names.matches(&e.exported))
            .map(Row::Export)
            .collect(),
    })
}
