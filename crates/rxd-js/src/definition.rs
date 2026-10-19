//! Extract the source declaration owning a canonical lexical binding identifier.

use oxc_ast::{AstKind, ast::Program};
use oxc_span::{GetSpan, Span};
use rxd_core::{Error, Limits, Result, Source, perf::Perf};
use serde::Serialize;

use crate::{
    lexical::{self, Location, Locations},
    strings::JsString,
    structure::Collector,
};

pub struct Symbol {
    id: String,
    name: JsString,
    pub location: Location,
    space: Option<&'static str>,
    declaration_ids: Option<Vec<String>>,
}

impl Symbol {
    pub fn envelope(&self, source_id: &str) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            name: &'a JsString,
            space: Option<&'a str>,
            declaration_ids: &'a Option<Vec<String>>,
        }
        crate::envelope::body(
            "binding",
            rxd_core::envelope::RecordMeta {
                id: Some(&self.id),
                source_id: Some(source_id),
                parent_id: Some(&self.location.entity_id),
                start: Some(self.location.start as usize),
                end: Some(self.location.end as usize),
                ..Default::default()
            },
            Detail {
                name: &self.name,
                space: self.space,
                declaration_ids: &self.declaration_ids,
            },
        )
    }
}

#[derive(Serialize)]
pub struct Range {
    pub start: u32,
    pub end: u32,
}

impl From<Span> for Range {
    fn from(span: Span) -> Self {
        Self {
            start: span.start,
            end: span.end,
        }
    }
}

#[derive(Serialize)]
pub struct Declaration {
    kind: &'static str,
    #[serde(flatten)]
    pub range: Range,
    variable_kind: Option<&'static str>,
    initializer: Option<Range>,
}

pub struct Definition {
    pub symbol: Symbol,
    pub declaration: Declaration,
}

fn validate_id(source: &Source, id: &str) -> Result<Span> {
    let parts: Vec<_> = id.split(':').collect();
    let invalid = || Error::new("INVALID_ID", "malformed or unsupported symbol ID");
    if parts.len() != 6
        || parts[0] != "rxd1"
        || parts[1] != "js"
        || parts[3] != "symbol"
        || parts[2].len() != 64
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(invalid());
    }
    let span = Span::new(
        parts[4].parse().map_err(|_| invalid())?,
        parts[5].parse().map_err(|_| invalid())?,
    );
    if span.start >= span.end {
        return Err(invalid());
    }
    if parts[2] != source.id {
        return Err(Error::new(
            "SOURCE_MISMATCH",
            "symbol ID belongs to different source content",
        ));
    }
    if source.entity_id("symbol", span.start, span.end) != id {
        return Err(invalid());
    }
    Ok(span)
}

pub fn run(
    program: &Program<'_>,
    source: &Source,
    collector: &Collector<'_>,
    id: &str,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Definition> {
    let span = perf.measure("query", || validate_id(source, id))?;
    let semantic = lexical::build(program, limits, perf)?;
    perf.measure("query", || {
        let nodes = semantic.nodes();
        // Validate the canonical identifier itself, not a later redeclaration or arbitrary slice.
        let (node_id, symbol_id) = nodes
            .iter_enumerated()
            .find_map(|(node_id, node)| {
                let (symbol, binding_span) = lexical::binding(node, &semantic)?;
                (binding_span == span && semantic.scoping().symbol_span(symbol) == span)
                    .then_some((node_id, symbol))
            })
            .ok_or_else(|| {
                Error::new(
                    "INVALID_ID",
                    "ID does not identify a canonical source binding",
                )
            })?;
        let declaration = std::iter::once((node_id, nodes.get_node(node_id)))
            .chain(nodes.ancestors_enumerated(node_id))
            .find_map(|(ancestor_id, node)| {
                let (kind, original, variable_kind, initializer) = match node.kind() {
                    AstKind::VariableDeclarator(declaration) => {
                        let AstKind::VariableDeclaration(parent) = nodes.parent_kind(ancestor_id)
                        else {
                            return None;
                        };
                        (
                            "variable_declarator",
                            declaration.span,
                            Some(parent.kind.as_str()),
                            declaration.init.as_ref().map(GetSpan::span),
                        )
                    }
                    AstKind::FormalParameter(parameter) => (
                        "parameter",
                        parameter.span,
                        None,
                        parameter
                            .initializer
                            .as_ref()
                            .map(|expression| expression.span()),
                    ),
                    AstKind::FormalParameterRest(parameter) => {
                        ("rest_parameter", parameter.span, None, None)
                    }
                    AstKind::CatchParameter(parameter) => {
                        ("catch_parameter", parameter.span, None, None)
                    }
                    AstKind::ImportDeclaration(import) => {
                        ("import_declaration", import.span, None, None)
                    }
                    AstKind::TSTypeAliasDeclaration(declaration) => {
                        ("type_alias", declaration.span, None, None)
                    }
                    AstKind::TSInterfaceDeclaration(declaration) => {
                        ("interface", declaration.span, None, None)
                    }
                    AstKind::TSEnumDeclaration(declaration) => {
                        ("enum", declaration.span, None, None)
                    }
                    AstKind::TSNamespaceDeclaration(declaration) => {
                        ("namespace", declaration.span, None, None)
                    }
                    AstKind::TSImportEqualsDeclaration(declaration) => {
                        ("import_equals_declaration", declaration.span, None, None)
                    }
                    AstKind::TSTypeParameter(parameter) => {
                        ("type_parameter", parameter.span, None, None)
                    }
                    AstKind::TSEnumMember(member) => (
                        "enum_member",
                        member.span,
                        None,
                        member.initializer.as_ref().map(GetSpan::span),
                    ),
                    AstKind::Function(function) => (
                        if function.is_declaration() {
                            "function_declaration"
                        } else {
                            "function_expression"
                        },
                        function.span,
                        None,
                        None,
                    ),
                    AstKind::Class(class) => (
                        if class.is_declaration() {
                            "class_declaration"
                        } else {
                            "class_expression"
                        },
                        class.span,
                        None,
                        None,
                    ),
                    _ => return None,
                };
                Some(Declaration {
                    kind,
                    range: original.into(),
                    variable_kind,
                    initializer: initializer.map(Into::into),
                })
            })
            .ok_or_else(|| {
                Error::new(
                    "INVALID_TARGET",
                    "binding has no supported source declaration",
                )
            })?;
        let location = Locations::new(collector).get(span);
        Ok(Definition {
            symbol: Symbol {
                id: id.to_owned(),
                name: lexical::name(&semantic, symbol_id),
                space: program
                    .source_type
                    .is_typescript()
                    .then(|| lexical::space(&semantic, symbol_id)),
                declaration_ids: lexical::declaration_ids(
                    &semantic,
                    symbol_id,
                    source,
                    Limits {
                        records: limits.records.saturating_sub(1),
                        ..limits
                    },
                )?,
                location,
            },
            declaration,
        })
    })
}
