//! Shared on-demand semantic analysis and original identifier locations.

use oxc_ast::{AstKind, ast::Program};
use oxc_semantic::{AstNode, Semantic, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, Span};
use rxd_core::{Error, Limits, Result, Source, perf::Perf};
use serde::Serialize;

use crate::{fragment, strings::JsString, structure::Collector};

/// Enum members are lexical bindings inside enum initializers, unlike ordinary properties.
pub fn binding<'a>(node: &AstNode<'a>, semantic: &Semantic<'a>) -> Option<(SymbolId, Span)> {
    match node.kind() {
        AstKind::BindingIdentifier(id) => id.symbol_id.get().map(|symbol| (symbol, id.span)),
        AstKind::TSEnumMember(member) => semantic
            .scoping()
            .get_binding(node.scope_id(), member.id.static_name().as_str().into())
            .map(|symbol| (symbol, member.id.span())),
        _ => None,
    }
}

pub fn name(semantic: &Semantic<'_>, symbol: SymbolId) -> JsString {
    if let AstKind::TSEnumMember(member) = semantic.symbol_declaration(symbol).kind() {
        match &member.id {
            oxc_ast::ast::TSEnumMemberName::String(literal)
            | oxc_ast::ast::TSEnumMemberName::ComputedString(literal) => {
                return JsString::decoded(literal.value.as_str(), literal.lone_surrogates);
            }
            oxc_ast::ast::TSEnumMemberName::ComputedTemplateString(template) => {
                if let Some(quasi) = template.quasis.first()
                    && let Some(cooked) = &quasi.value.cooked
                {
                    return JsString::decoded(cooked.as_str(), quasi.lone_surrogates);
                }
            }
            _ => {}
        }
    }
    JsString::from(semantic.scoping().symbol_name(symbol))
}

pub fn space(semantic: &Semantic<'_>, symbol: SymbolId) -> &'static str {
    let flags = semantic.scoping().symbol_flags(symbol);
    match (flags.can_be_referenced_by_type(), flags.is_value()) {
        (true, true) => "type_value",
        (true, false) => "type",
        _ => "value",
    }
}

/// Navigation to every merged/redeclared TS source declaration, under the record budget.
pub fn declaration_ids(
    semantic: &Semantic<'_>,
    symbol: SymbolId,
    source: &Source,
) -> Result<Option<Vec<String>>> {
    if !semantic.source_type().is_typescript() {
        return Ok(None);
    }
    let redeclarations = semantic.scoping().symbol_redeclarations(symbol);
    if redeclarations.len() < 2 {
        return Ok(None);
    }
    let mut result = Vec::new();
    for declaration in redeclarations {
        let node = semantic.nodes().kind(declaration.declaration);
        let span = node.span();
        let id = match node {
            AstKind::Function(function) => source.entity_id(
                if function.is_declaration() {
                    "function_declaration"
                } else {
                    "function_expression"
                },
                span.start,
                span.end,
            ),
            AstKind::Class(class) => source.entity_id(
                if class.is_declaration() {
                    "class_declaration"
                } else {
                    "class_expression"
                },
                span.start,
                span.end,
            ),
            AstKind::TSTypeAliasDeclaration(_) => {
                source.entity_id("type_alias", span.start, span.end)
            }
            AstKind::TSInterfaceDeclaration(_) => {
                source.entity_id("interface", span.start, span.end)
            }
            AstKind::TSEnumDeclaration(_) => source.entity_id("enum", span.start, span.end),
            AstKind::TSNamespaceDeclaration(_) => {
                source.entity_id("namespace", span.start, span.end)
            }
            _ => fragment::key(node)
                .map(|key| key.id(source))
                .ok_or_else(|| {
                    Error::new(
                        "NOT_FOUND",
                        "merged declaration has no source navigation target",
                    )
                })?,
        };
        result.push(id);
    }
    result.dedup();
    Ok(Some(result))
}

#[derive(Serialize)]
pub struct Location {
    pub start: u32,
    pub end: u32,
    #[serde(serialize_with = "crate::envelope::serialize_short")]
    pub entity_id: String,
}

pub fn build<'a>(
    program: &'a Program<'a>,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Semantic<'a>> {
    let semantic = perf.measure("semantic", || -> Result<_> {
        let stats = oxc_semantic::Stats::count(program);
        if stats.nodes as usize > limits.semantic_nodes {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "semantic node budget exceeded",
            ));
        }
        let built = SemanticBuilder::new_compiler()
            .with_build_nodes(true)
            .with_stats(stats)
            .build(program);
        if !built.diagnostics.is_empty() {
            return Err(Error::new(
                "INVALID_INPUT",
                built
                    .diagnostics
                    .iter()
                    .take(3)
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        Ok(built.semantic)
    })?;
    perf.counters.semantic_nodes = Some(semantic.nodes().len());
    perf.counters.semantic_symbols = Some(semantic.scoping().symbols_len());
    perf.counters.semantic_references = Some(semantic.scoping().references_len());
    Ok(semantic)
}

pub fn is_dynamic(semantic: &Semantic<'_>) -> bool {
    semantic.nodes().iter().any(|node| match node.kind() {
        AstKind::WithStatement(_) => true,
        AstKind::CallExpression(call) => !call.optional && call.callee.is_specific_id("eval"),
        _ => false,
    })
}

pub struct Locations<'c, 's> {
    collector: &'c Collector<'s>,
    ordered: Vec<usize>,
}

impl<'c, 's> Locations<'c, 's> {
    pub fn new(collector: &'c Collector<'s>) -> Self {
        let mut ordered = collector.ordered_entities();
        ordered.sort_by_key(|&i| {
            let entity = &collector.entities[i].entity;
            (entity.start, std::cmp::Reverse(entity.end), entity.depth)
        });
        Self { collector, ordered }
    }

    pub fn get(&self, span: Span) -> Location {
        let index = self
            .ordered
            .partition_point(|&i| self.collector.entities[i].entity.start <= span.start);
        let mut owner = self.ordered[index - 1];
        loop {
            let node = &self.collector.entities[owner];
            if node.entity.start <= span.start && span.end <= node.entity.end {
                return Location {
                    start: span.start,
                    end: span.end,
                    entity_id: node.entity.id.clone(),
                };
            }
            owner = node.parent.expect("program contains identifier span");
        }
    }
}
