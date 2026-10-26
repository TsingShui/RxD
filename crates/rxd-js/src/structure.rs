use std::{
    collections::{HashMap, HashSet},
    panic::panic_any,
};

use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};
use rxd_core::{Error, Limits, Result, Source};
use serde::Serialize;

use crate::{decode::Decoders, fragment::Contexts, static_eval, strings::JsString};

#[derive(Default, Serialize)]
pub struct Flags {
    pub r#async: bool,
    pub generator: bool,
    pub r#static: bool,
}

pub struct Entity {
    pub id: String,
    pub kind: &'static str,
    pub name: Option<JsString>,
    pub label: Option<JsString>,
    pub label_origin: &'static str,
    pub start: u32,
    pub end: u32,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub flags: Flags,
    pub key_span: Option<SpanRecord>,
}

#[derive(Serialize)]
pub struct SpanRecord {
    start: u32,
    end: u32,
}

pub struct EntityNode {
    pub entity: Entity,
    pub parent: Option<usize>,
}

pub struct Match {
    pub literal_kind: &'static str,
    pub value: JsString,
    pub start: u32,
    pub end: u32,
    pub dynamic: bool,
    pub resolution: Option<&'static str>,
    pub raw: Option<String>,
    pub entity_id: String,
    pub context_id: Option<String>,
}

impl Entity {
    pub fn envelope(&self) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            kind: &'a str,
            name: &'a Option<JsString>,
            label: &'a Option<JsString>,
            label_origin: &'a str,
            depth: usize,
            flags: &'a Flags,
            key_span: &'a Option<SpanRecord>,
        }
        crate::envelope::body(
            "entity",
            rxd_core::envelope::RecordMeta {
                at: Some(&self.id),
                parent_at: self.parent_id.as_deref(),
                start: Some(self.start as usize),
                end: Some(self.end as usize),
                ..Default::default()
            },
            Detail {
                kind: self.kind,
                name: &self.name,
                label: &self.label,
                label_origin: self.label_origin,
                depth: self.depth,
                flags: &self.flags,
                key_span: &self.key_span,
            },
        )
    }
}

impl Match {
    pub fn envelope(&self) -> impl Serialize {
        #[derive(Serialize)]
        struct Detail<'a> {
            literal_kind: &'a str,
            value: &'a JsString,
            dynamic: bool,
            resolution: Option<&'static str>,
            raw: &'a Option<String>,
        }
        crate::envelope::body(
            "match",
            rxd_core::envelope::RecordMeta {
                parent_at: Some(&self.entity_id),
                context_at: self.context_id.as_deref(),
                start: Some(self.start as usize),
                end: Some(self.end as usize),
                ..Default::default()
            },
            Detail {
                literal_kind: self.literal_kind,
                value: &self.value,
                dynamic: self.dynamic,
                resolution: self.resolution,
                raw: &self.raw,
            },
        )
    }
}

pub struct Collector<'s> {
    source: &'s Source,
    pub(super) limits: Limits,
    query: Option<Vec<u16>>,
    query_case_sensitive: bool,
    pub entities: Vec<EntityNode>,
    pub matches: Vec<Match>,
    match_budget: crate::names::RecordBudget,
    pub contexts: Option<Contexts>,
    stack: Vec<usize>,
    frames: Vec<bool>,
    suppressed: HashSet<(u32, u32)>,
    labels: HashMap<(u32, u32), (JsString, &'static str)>,
    keys: HashSet<(u32, u32)>,
    dynamic_templates: HashSet<(u32, u32)>,
    jsx_strings: HashSet<(u32, u32)>,
    type_strings: HashSet<(u32, u32)>,
    type_templates: HashSet<(u32, u32)>,
    resolve_static: bool,
    decoders: Option<&'s Decoders>,
    pub static_limits: static_eval::Limits,
}

impl<'s> Collector<'s> {
    pub fn new(
        source: &'s Source,
        limits: Limits,
        query: Option<&str>,
        query_case_sensitive: bool,
        contexts: Option<Contexts>,
        resolve_static: bool,
        decoders: Option<&'s Decoders>,
    ) -> Self {
        Self {
            source,
            limits,
            // Fold the needle once; the per-literal test only folds the haystack.
            query: query.map(|q| {
                if query_case_sensitive {
                    q.encode_utf16().collect()
                } else {
                    q.to_lowercase().encode_utf16().collect()
                }
            }),
            query_case_sensitive,
            entities: Vec::new(),
            matches: Vec::new(),
            match_budget: crate::names::RecordBudget::new(limits),
            contexts,
            stack: Vec::new(),
            frames: Vec::new(),
            suppressed: HashSet::new(),
            labels: HashMap::new(),
            keys: HashSet::new(),
            dynamic_templates: HashSet::new(),
            jsx_strings: HashSet::new(),
            type_strings: HashSet::new(),
            type_templates: HashSet::new(),
            resolve_static,
            decoders,
            static_limits: static_eval::Limits::default(),
        }
    }

    fn add(
        &mut self,
        node: AstKind<'_>,
        kind: &'static str,
        name: Option<JsString>,
        flags: Flags,
        key: Option<Span>,
    ) {
        let span = if matches!(node, AstKind::Program(_)) {
            Span::new(0, self.source.text.len() as u32)
        } else {
            node.span()
        };
        let context = self
            .labels
            .get(&(span.start, span.end))
            .cloned()
            .or_else(|| key.and_then(|key| self.labels.get(&(key.start, key.end)).cloned()))
            .or_else(|| {
                key.and_then(|key| self.decoders.and_then(|d| d.at_span(key)))
                    .map(|v| (v.value.clone(), "decoder"))
            });
        let (label, label_origin) = if let Some(name) = &name {
            (Some(name.clone()), "declared")
        } else if let Some((label, origin)) = context {
            (Some(label), origin)
        } else {
            (None, "anonymous")
        };
        let parent = self.stack.last().copied();
        let parent_id = parent.map(|p| self.entities[p].entity.id.clone());
        let entity = Entity {
            id: self.source.entity_id(kind, span.start, span.end),
            kind,
            name,
            label,
            label_origin,
            start: span.start,
            end: span.end,
            parent_id,
            depth: self.stack.len(),
            flags,
            key_span: key.map(|s| SpanRecord {
                start: s.start,
                end: s.end,
            }),
        };
        self.stack.push(self.entities.len());
        self.entities.push(EntityNode { entity, parent });
    }

    fn label(&mut self, expression: &Expression<'_>, name: Option<JsString>, origin: &'static str) {
        let mut expression = expression.without_parentheses();
        loop {
            expression = match expression {
                Expression::TSAsExpression(wrapper) => &wrapper.expression,
                Expression::TSSatisfiesExpression(wrapper) => &wrapper.expression,
                Expression::TSNonNullExpression(wrapper) => &wrapper.expression,
                Expression::TSTypeAssertion(wrapper) => &wrapper.expression,
                Expression::TSInstantiationExpression(wrapper) => &wrapper.expression,
                _ => break,
            }
            .without_parentheses();
        }
        if !matches!(
            expression,
            Expression::FunctionExpression(_)
                | Expression::ArrowFunctionExpression(_)
                | Expression::ClassExpression(_)
        ) {
            return;
        }
        if let Some(name) = name {
            let span = expression.span();
            self.labels.insert((span.start, span.end), (name, origin));
        }
    }

    fn literal(
        &mut self,
        span: Span,
        value: JsString,
        literal_kind: &'static str,
        dynamic: bool,
        resolution: Option<&'static str>,
    ) {
        if !self.query.as_ref().is_some_and(|query| {
            value.contains_prepared(query.as_slice(), self.query_case_sensitive)
        }) {
            return;
        }
        self.match_budget
            .charge(&value)
            .and_then(|_| {
                self.match_budget.charge_bytes(
                    if matches!(literal_kind, "static_expression" | "decoded") {
                        span.size() as usize
                    } else {
                        0
                    },
                )
            })
            .unwrap_or_else(|e| panic_any(e));
        let parent = *self.stack.last().expect("program entity precedes literals");
        self.matches.push(Match {
            literal_kind,
            value,
            start: span.start,
            end: span.end,
            dynamic,
            resolution,
            raw: matches!(literal_kind, "static_expression" | "decoded")
                .then(|| self.source.text[span.start as usize..span.end as usize].to_owned()),
            entity_id: self.entities[parent].entity.id.clone(),
            context_id: self
                .contexts
                .as_ref()
                .and_then(|contexts| contexts.current_id(self.source)),
        });
    }

    pub fn evaluation(&self) -> static_eval::Context<'_> {
        static_eval::Context {
            limits: self.static_limits,
            decoders: self.decoders,
        }
    }
    fn key_label(
        &mut self,
        key: &PropertyKey<'_>,
        computed: bool,
    ) -> Option<(JsString, &'static str)> {
        if !computed || !self.resolve_static {
            return None;
        }
        let (value, span, origin) =
            crate::members::computed_key(key.as_expression()?, self.decoders)?;
        if origin != "decoder" {
            return None;
        }
        // Admit the key cache, optional function context and eventual entity label together.
        self.match_budget
            .charge_bytes(value.len_utf16().saturating_mul(6).saturating_add(384))
            .unwrap_or_else(|e| panic_any(e));
        self.labels
            .insert((span.start, span.end), (value.clone(), origin));
        Some((value, origin))
    }
    fn evaluated_literal(&mut self, span: Span, evaluated: Option<static_eval::Evaluated>) {
        if let Some(value) = evaluated {
            let resolution = value.resolution();
            if let static_eval::Value::String(value) = value.value {
                self.literal(span, value, "static_expression", false, Some(resolution));
            }
        }
    }

    /// Decoders are built once per run and shared by every query that resolves keys.
    pub fn decoders(&self) -> Option<&Decoders> {
        self.decoders
    }

    pub fn ordered_entities(&self) -> Vec<usize> {
        let mut indices: Vec<_> = (0..self.entities.len()).collect();
        indices.sort_by_key(|&i| {
            let e = &self.entities[i].entity;
            (e.start, std::cmp::Reverse(e.end), e.kind)
        });
        indices
    }

    pub fn by_id(&self, id: &str) -> Result<usize> {
        let parts: Vec<_> = id.split(':').collect();
        if parts.len() != 3 || parts[1].parse::<u32>().is_err() || parts[2].parse::<u32>().is_err()
        {
            return Err(Error::new(
                "INVALID_QUERY",
                "malformed or unsupported coordinate",
            ));
        }
        // A coordinate names a span, not an input.
        self.entities
            .iter()
            .position(|e| e.entity.id == id)
            .ok_or_else(|| {
                rxd_core::Error::failure(
                    rxd_core::Failure::NotFound,
                    "coordinate does not identify a real structure",
                )
            })
    }

    pub fn is_descendant(&self, candidate: usize, root: usize) -> bool {
        let mut current = self.entities[candidate].parent;
        while let Some(i) = current {
            if i == root {
                return true;
            }
            current = self.entities[i].parent;
        }
        false
    }
}

fn key_name(key: &PropertyKey<'_>, computed: bool) -> Option<JsString> {
    if computed {
        return None;
    }
    match key {
        PropertyKey::StringLiteral(literal) => Some(JsString::decoded(
            literal.value.as_str(),
            literal.lone_surrogates,
        )),
        PropertyKey::PrivateIdentifier(identifier) => {
            Some(JsString::from(format!("#{}", identifier.name).as_str()))
        }
        _ => key.static_name().map(|name| JsString::from(name.as_ref())),
    }
}

fn function_flags(function: &Function<'_>, is_static: bool) -> Flags {
    Flags {
        r#async: function.r#async,
        generator: function.generator,
        r#static: is_static,
    }
}

impl<'a> Visit<'a> for Collector<'_> {
    fn enter_node(&mut self, node: AstKind<'a>) {
        if self.frames.len() >= self.limits.ast_depth {
            panic_any(Error::new("RESOURCE_LIMIT", "AST depth budget exceeded"));
        }
        let old_len = self.stack.len();
        // Context nodes do not create structure entities; retain only the active AST chain.
        if let Some(contexts) = &mut self.contexts {
            contexts.enter(node, self.stack.last().copied().unwrap_or(0));
        }
        match node {
            AstKind::Program(_) => self.add(node, "program", None, Flags::default(), None),
            AstKind::VariableDeclarator(decl) => {
                if let Some(expr) = &decl.init {
                    self.label(
                        expr,
                        decl.id
                            .get_binding_identifier()
                            .map(|id| JsString::from(id.name.as_str())),
                        "binding",
                    );
                }
            }
            AstKind::AssignmentExpression(assignment) => {
                self.label(
                    &assignment.right,
                    assignment.left.get_identifier_name().map(JsString::from),
                    "assignment",
                );
            }
            AstKind::ObjectProperty(property) => {
                let key = property.key.span();
                self.keys.insert((key.start, key.end));
                let name = key_name(&property.key, property.computed);
                let decoded = self.key_label(&property.key, property.computed);
                if property.method || property.kind != PropertyKind::Init {
                    let Expression::FunctionExpression(function) = &property.value else {
                        unreachable!("method function")
                    };
                    self.suppressed
                        .insert((function.span.start, function.span.end));
                    let kind = match property.kind {
                        PropertyKind::Init => "object_method",
                        PropertyKind::Get => "getter",
                        PropertyKind::Set => "setter",
                    };
                    self.add(
                        node,
                        kind,
                        name,
                        function_flags(function, false),
                        property.computed.then_some(key),
                    );
                } else {
                    let origin = decoded.as_ref().map_or("property", |v| v.1);
                    let name = name.or_else(|| decoded.map(|v| v.0));
                    self.label(&property.value, name, origin);
                }
            }
            AstKind::PropertyDefinition(property) => {
                let key = property.key.span();
                self.keys.insert((key.start, key.end));
                let decoded = self.key_label(&property.key, property.computed);
                if let Some(value) = &property.value {
                    let origin = decoded.as_ref().map_or("property", |v| v.1);
                    self.label(
                        value,
                        key_name(&property.key, property.computed).or_else(|| decoded.map(|v| v.0)),
                        origin,
                    );
                }
            }
            AstKind::MethodDefinition(method) => {
                let key = method.key.span();
                self.keys.insert((key.start, key.end));
                self.key_label(&method.key, method.computed);
                self.suppressed
                    .insert((method.value.span.start, method.value.span.end));
                let kind = match method.kind {
                    MethodDefinitionKind::Constructor => "constructor",
                    MethodDefinitionKind::Method => "class_method",
                    MethodDefinitionKind::Get => "getter",
                    MethodDefinitionKind::Set => "setter",
                };
                self.add(
                    node,
                    kind,
                    key_name(&method.key, method.computed),
                    function_flags(&method.value, method.r#static),
                    method.computed.then_some(key),
                );
            }
            AstKind::Function(function) => {
                if !self
                    .suppressed
                    .contains(&(function.span.start, function.span.end))
                {
                    let kind = if function.is_declaration() {
                        "function_declaration"
                    } else {
                        "function_expression"
                    };
                    self.add(
                        node,
                        kind,
                        function
                            .id
                            .as_ref()
                            .map(|id| JsString::from(id.name.as_str())),
                        function_flags(function, false),
                        None,
                    );
                }
            }
            AstKind::ArrowFunctionExpression(arrow) => self.add(
                node,
                "arrow_function",
                None,
                Flags {
                    r#async: arrow.r#async,
                    ..Flags::default()
                },
                None,
            ),
            AstKind::Class(class) => {
                let kind = if class.r#type == ClassType::ClassDeclaration {
                    "class_declaration"
                } else {
                    "class_expression"
                };
                self.add(
                    node,
                    kind,
                    class.id.as_ref().map(|id| JsString::from(id.name.as_str())),
                    Flags::default(),
                    None,
                );
            }
            AstKind::TSTypeAliasDeclaration(declaration) => self.add(
                node,
                "type_alias",
                Some(JsString::from(declaration.id.name.as_str())),
                Flags::default(),
                None,
            ),
            AstKind::TSInterfaceDeclaration(declaration) => self.add(
                node,
                "interface",
                Some(JsString::from(declaration.id.name.as_str())),
                Flags::default(),
                None,
            ),
            AstKind::TSEnumDeclaration(declaration) => self.add(
                node,
                "enum",
                Some(JsString::from(declaration.id.name.as_str())),
                Flags::default(),
                None,
            ),
            AstKind::TSNamespaceDeclaration(declaration) => self.add(
                node,
                "namespace",
                Some(JsString::from(declaration.id.name.as_str())),
                Flags::default(),
                None,
            ),
            AstKind::TSExternalModuleDeclaration(declaration) => self.add(
                node,
                "external_module",
                Some(JsString::decoded(
                    declaration.id.value.as_str(),
                    declaration.id.lone_surrogates,
                )),
                Flags::default(),
                None,
            ),
            AstKind::TSGlobalDeclaration(_) => self.add(
                node,
                "global_declaration",
                Some(JsString::from("global")),
                Flags::default(),
                None,
            ),
            AstKind::TSMethodSignature(method) => self.add(
                node,
                "method_signature",
                key_name(&method.key, method.computed),
                Flags::default(),
                method.computed.then_some(method.key.span()),
            ),
            AstKind::JSXAttribute(attribute) if self.query.is_some() => {
                if let Some(JSXAttributeValue::StringLiteral(literal)) = &attribute.value {
                    self.jsx_strings
                        .insert((literal.span.start, literal.span.end));
                }
            }
            AstKind::JSXText(text) if self.query.is_some() => self.literal(
                text.span,
                JsString::jsx(text.value.as_str()),
                "jsx_text",
                false,
                None,
            ),
            AstKind::TSLiteralType(literal) if self.query.is_some() => {
                if let TSLiteral::StringLiteral(string) = &literal.literal {
                    self.type_strings
                        .insert((string.span.start, string.span.end));
                }
            }
            AstKind::TSTemplateLiteralType(template) if self.query.is_some() => {
                for quasi in &template.quasis {
                    self.type_templates
                        .insert((quasi.span.start, quasi.span.end));
                    if !template.types.is_empty() {
                        self.dynamic_templates
                            .insert((quasi.span.start, quasi.span.end));
                    }
                }
            }
            AstKind::BinaryExpression(binary)
                if self.query.is_some()
                    && self.resolve_static
                    && static_eval::binary_can_yield_string(binary) =>
            {
                self.evaluated_literal(binary.span, self.evaluation().binary(binary));
            }
            AstKind::CallExpression(call) if self.query.is_some() && self.resolve_static => {
                let decoders = self.decoders;
                let decoded = decoders
                    .and_then(|decoders| decoders.call(call))
                    .map(|resolved| resolved.value.clone());
                if let Some(value) = decoded {
                    self.literal(call.span, value, "decoded", false, Some("decoder"));
                } else if static_eval::call_can_yield_string(call) {
                    self.evaluated_literal(call.span, self.evaluation().call(call));
                }
            }
            AstKind::TemplateLiteral(template)
                if self.query.is_some()
                    && self.resolve_static
                    && static_eval::template_can_yield_string(template) =>
            {
                self.evaluated_literal(template.span, self.evaluation().template(template));
            }
            AstKind::ComputedMemberExpression(member)
                if self.query.is_some()
                    && self.resolve_static
                    && static_eval::member_can_yield_string(member) =>
            {
                self.evaluated_literal(member.span, self.evaluation().member(member));
            }
            AstKind::StringLiteral(literal) if self.query.is_some() => {
                let span = (literal.span.start, literal.span.end);
                let kind = if self.jsx_strings.contains(&span) {
                    "jsx_attribute"
                } else if self.type_strings.contains(&span) {
                    "type_string"
                } else if self.keys.contains(&span) {
                    "property_key"
                } else {
                    "string"
                };
                self.literal(
                    literal.span,
                    if kind == "jsx_attribute" {
                        JsString::jsx(literal.value.as_str())
                    } else {
                        JsString::decoded(literal.value.as_str(), literal.lone_surrogates)
                    },
                    kind,
                    false,
                    None,
                );
            }
            AstKind::TemplateLiteral(template) if self.query.is_some() => {
                if !template.expressions.is_empty() {
                    for quasi in &template.quasis {
                        self.dynamic_templates
                            .insert((quasi.span.start, quasi.span.end));
                    }
                }
            }
            AstKind::TemplateElement(quasi) if self.query.is_some() => {
                if let Some(cooked) = &quasi.value.cooked {
                    self.literal(
                        quasi.span,
                        JsString::decoded(cooked.as_str(), quasi.lone_surrogates),
                        if self
                            .type_templates
                            .contains(&(quasi.span.start, quasi.span.end))
                        {
                            "type_template"
                        } else {
                            "template"
                        },
                        self.dynamic_templates
                            .contains(&(quasi.span.start, quasi.span.end)),
                        None,
                    );
                }
            }
            _ => {}
        }
        self.frames.push(self.stack.len() > old_len);
    }

    fn leave_node(&mut self, _node: AstKind<'a>) {
        if let Some(contexts) = &mut self.contexts {
            contexts.leave();
        }
        if self.frames.pop() == Some(true) {
            self.stack.pop();
        }
    }
}
