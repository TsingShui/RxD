//! Bounded local syntax contexts, shared by parser traversal and lexical references.

use oxc_ast::{AstKind, ast::PropertyKind};
use oxc_span::{GetSpan, Span};
use rxd_core::{Error, Result, Source};

use crate::structure::Collector;

#[derive(Clone, Copy)]
pub struct Key {
    kind: &'static str,
    span: Span,
}

pub fn key(node: AstKind<'_>) -> Option<Key> {
    let kind = match node {
        AstKind::VariableDeclaration(_) => "variable_declaration",
        AstKind::VariableDeclarator(_) => "variable_declarator",
        AstKind::AssignmentExpression(_) => "assignment_expression",
        AstKind::UpdateExpression(_) => "update_expression",
        AstKind::ObjectProperty(property)
            if !property.method && property.kind == PropertyKind::Init =>
        {
            "object_property"
        }
        AstKind::PropertyDefinition(_) => "class_field",
        AstKind::FormalParameter(_) => "parameter",
        AstKind::FormalParameterRest(_) => "rest_parameter",
        AstKind::CatchParameter(_) => "catch_parameter",
        AstKind::ImportDeclaration(_) => "import_declaration",
        AstKind::ExportNamedDeclaration(_) => "export_named_declaration",
        AstKind::ExportDefaultDeclaration(_) => "export_default_declaration",
        AstKind::ExportAllDeclaration(_) => "export_all_declaration",
        AstKind::ExpressionStatement(_) => "expression_statement",
        AstKind::ReturnStatement(_) => "return_statement",
        AstKind::ThrowStatement(_) => "throw_statement",
        AstKind::IfStatement(_) => "if_statement",
        AstKind::SwitchStatement(_) => "switch_statement",
        AstKind::SwitchCase(_) => "switch_case",
        AstKind::ForStatement(_) => "for_statement",
        AstKind::ForInStatement(_) => "for_in_statement",
        AstKind::ForOfStatement(_) => "for_of_statement",
        AstKind::WhileStatement(_) => "while_statement",
        AstKind::DoWhileStatement(_) => "do_while_statement",
        AstKind::TryStatement(_) => "try_statement",
        AstKind::WithStatement(_) => "with_statement",
        AstKind::LabeledStatement(_) => "labeled_statement",
        AstKind::BreakStatement(_) => "break_statement",
        AstKind::ContinueStatement(_) => "continue_statement",
        AstKind::DebuggerStatement(_) => "debugger_statement",
        AstKind::EmptyStatement(_) => "empty_statement",
        AstKind::JSXElement(_) => "jsx_element",
        AstKind::JSXFragment(_) => "jsx_fragment",
        AstKind::JSXOpeningElement(_) => "jsx_opening_element",
        AstKind::JSXClosingElement(_) => "jsx_closing_element",
        AstKind::JSXAttribute(_) => "jsx_attribute",
        AstKind::JSXSpreadAttribute(_) => "jsx_spread_attribute",
        AstKind::JSXExpressionContainer(_) => "jsx_expression",
        AstKind::JSXText(_) => "jsx_text",
        AstKind::TSTypeParameter(_) => "type_parameter",
        AstKind::TSPropertySignature(_) => "property_signature",
        AstKind::TSCallSignatureDeclaration(_) => "call_signature",
        AstKind::TSConstructSignatureDeclaration(_) => "construct_signature",
        AstKind::TSIndexSignature(_) => "index_signature",
        AstKind::TSFunctionType(_) => "function_type",
        AstKind::TSConstructorType(_) => "constructor_type",
        AstKind::TSEnumMember(_) => "enum_member",
        AstKind::TSImportEqualsDeclaration(_) => "import_equals_declaration",
        AstKind::TSExportAssignment(_) => "export_assignment",
        _ => return None,
    };
    Some(Key {
        kind,
        span: node.span(),
    })
}

pub fn boundary(node: AstKind<'_>) -> bool {
    matches!(
        node,
        AstKind::Program(_)
            | AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::Class(_)
            | AstKind::MethodDefinition(_)
            | AstKind::TSTypeAliasDeclaration(_)
            | AstKind::TSInterfaceDeclaration(_)
            | AstKind::TSNamespaceDeclaration(_)
            | AstKind::TSExternalModuleDeclaration(_)
            | AstKind::TSGlobalDeclaration(_)
            | AstKind::TSEnumDeclaration(_)
            | AstKind::TSMethodSignature(_)
    ) || matches!(node, AstKind::ObjectProperty(property) if property.method || property.kind != PropertyKind::Init)
}

impl Key {
    pub fn id(self, source: &Source) -> String {
        source.entity_id(
            &format!("fragment.{}", self.kind),
            self.span.start,
            self.span.end,
        )
    }
}

#[derive(Clone, Copy)]
struct Selection {
    key: Key,
    parent: Option<Key>,
    entity: usize,
}

pub enum Target {
    Matches,
    Byte(u64),
    Id(String),
}

pub struct Fragment {
    pub id: String,
    pub kind: &'static str,
    pub start: u32,
    pub end: u32,
    pub parent_id: Option<String>,
    pub entity_id: String,
}

pub struct Contexts {
    target: Target,
    wanted: Option<(String, Span)>,
    current: Option<Selection>,
    frames: Vec<Option<Selection>>,
    selected: Option<Selection>,
}

impl Contexts {
    pub fn new(target: Target) -> Self {
        // Proper identity validation happens after the bounded traversal, like structure get.
        let wanted = if let Target::Id(id) = &target {
            let parts: Vec<_> = id.split(':').collect();
            if parts.len() == 6 {
                parts[3].strip_prefix("fragment.").and_then(|kind| {
                    let start = parts[4].parse().ok()?;
                    let end = parts[5].parse().ok()?;
                    (start < end).then(|| (kind.to_owned(), Span::new(start, end)))
                })
            } else {
                None
            }
        } else {
            None
        };
        Self {
            target,
            wanted,
            current: None,
            frames: Vec::new(),
            selected: None,
        }
    }

    pub fn enter(&mut self, node: AstKind<'_>, entity: usize) {
        self.frames.push(self.current);
        if boundary(node) {
            self.current = None;
            if let Target::Byte(byte) = self.target {
                let span = node.span();
                if u64::from(span.start) <= byte && byte < u64::from(span.end) {
                    self.selected = None;
                }
            }
        } else if let Some(key) = key(node) {
            let selection = Selection {
                key,
                parent: self.current.map(|s| s.key),
                entity,
            };
            self.current = Some(selection);
            let selected = match &self.target {
                Target::Matches => false,
                Target::Byte(byte) => {
                    u64::from(key.span.start) <= *byte && *byte < u64::from(key.span.end)
                }
                Target::Id(_) => self
                    .wanted
                    .as_ref()
                    .is_some_and(|(kind, span)| kind == key.kind && *span == key.span),
            };
            if selected {
                self.selected = Some(selection);
            }
        }
    }

    pub fn leave(&mut self) {
        self.current = self.frames.pop().expect("balanced context traversal");
    }

    pub fn current_id(&self, source: &Source) -> Option<String> {
        self.current.map(|selection| selection.key.id(source))
    }

    pub fn selected(&self, collector: &Collector<'_>, source: &Source) -> Option<Fragment> {
        self.selected.map(|selection| Fragment {
            id: selection.key.id(source),
            kind: selection.key.kind,
            start: selection.key.span.start,
            end: selection.key.span.end,
            parent_id: selection
                .parent
                .map(|key| crate::envelope::short(&key.id(source))),
            entity_id: crate::envelope::short(&collector.entities[selection.entity].entity.id),
        })
    }
}

pub fn validate_id(source: &Source, id: &str) -> Result<()> {
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 6
        || parts[0] != "rxd1"
        || parts[1] != "js"
        || !parts[3].starts_with("fragment.")
        || parts[2].len() != 64
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || parts[4].parse::<u32>().is_err()
        || parts[5].parse::<u32>().is_err()
    {
        return Err(Error::new(
            "INVALID_ID",
            "malformed or unsupported fragment ID",
        ));
    }
    if parts[2] != source.id {
        return Err(Error::new(
            "SOURCE_MISMATCH",
            "fragment ID belongs to different source content",
        ));
    }
    Ok(())
}
