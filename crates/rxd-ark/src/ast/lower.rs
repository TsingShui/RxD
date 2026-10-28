use super::nodes::Ast;
use crate::{
    Module,
    analysis::Values,
    ir::*,
    reader::{invalid, resource},
    structure::{Body, Choice, Condition, LoopContext, Node},
};
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_span::{GetSpanMut, SPAN, SourceType, Span};
use oxc_str::Ident;
use oxc_syntax::operator::{
    BinaryOperator as B, LogicalOperator as L, UnaryOperator as U, UpdateOperator,
};
use rxd_core::{Budget, Result, output::Output};
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashSet},
    io::Write,
};

pub(crate) struct Prepared<'a> {
    pub function: crate::ir::Function<'a>,
    pub values: Values,
    pub body: Body,
}

struct SourceWarnings {
    next_anchor: u32,
    source: String,
    kinds: Vec<crate::ReconstructionWarning>,
    metadata_bytes: usize,
    comments: Vec<Comment>,
}

impl SourceWarnings {
    fn attach(
        &mut self,
        statement: &mut Statement<'_>,
        warning: crate::ReconstructionWarning,
    ) -> Result<()> {
        let current = statement.span_mut().start;
        let anchor = if self
            .comments
            .last()
            .is_some_and(|comment| comment.attached_to == current)
        {
            current
        } else {
            let anchor = self.next_anchor;
            self.next_anchor = anchor
                .checked_add(1)
                .ok_or_else(|| resource("Ark warning anchor exceeds span range"))?;
            *statement.span_mut() = Span::new(anchor, self.next_anchor);
            anchor
        };
        self.comments
            .len()
            .checked_add(1)
            .and_then(|count| count.checked_mul(std::mem::size_of::<Comment>()))
            .filter(|bytes| *bytes <= self.metadata_bytes)
            .ok_or_else(|| resource("Ark warning metadata admission exceeded"))?;
        self.comments
            .try_reserve(1)
            .map_err(|_| resource("Ark warning allocation failed"))?;
        let text = format!("// warning[{}]: {}", warning.code(), warning.message());
        let start = u32::try_from(self.source.len())
            .map_err(|_| resource("Ark warning source exceeds span range"))?;
        let end = start
            .checked_add(text.len() as u32)
            .filter(|end| *end as usize <= self.metadata_bytes)
            .ok_or_else(|| resource("Ark warning source admission exceeded"))?;
        self.source
            .try_reserve(text.len())
            .map_err(|_| resource("Ark warning source allocation failed"))?;
        self.source.push_str(&text);
        if !self.kinds.contains(&warning) {
            self.kinds.push(warning);
        }
        let mut comment = Comment::new(start, end, CommentKind::Line);
        comment.attached_to = anchor;
        comment.position = CommentPosition::Leading;
        comment.newlines = CommentNewlines::Leading;
        self.comments.push(comment);
        Ok(())
    }
}

pub(crate) fn lower(
    units: &[Prepared<'_>],
    module: Option<&Module>,
    budget: Budget,
    main: Option<u32>,
    unverified_version: bool,
) -> Result<(Vec<u8>, Vec<crate::ReconstructionWarning>)> {
    let arena = Allocator::new();
    let ast = Ast::new(&arena);
    let mut statements = imports(&ast, units, module)?;
    let uses_standard_iterator = units.iter().any(|unit| {
        unit.function
            .values
            .iter()
            .enumerate()
            .any(|(index, value)| unit.values.needed[index] && matches!(value.op, Op::Iterator(_)))
    });
    let max_code_len = units
        .iter()
        .map(|unit| unit.function.code.bytes.len())
        .max()
        .unwrap_or(0);
    // Bytecode PCs are below the code length, leaving unique synthetic comment anchors.
    let next_anchor = u32::try_from(max_code_len)
        .ok()
        .and_then(|len| len.checked_add(1))
        .ok_or_else(|| resource("Ark warning anchor exceeds span range"))?;
    let warnings = RefCell::new(SourceWarnings {
        next_anchor,
        source: String::new(),
        kinds: Vec::new(),
        metadata_bytes: budget.metadata_bytes,
        comments: Vec::new(),
    });
    let has_op = |predicate: fn(&Op) -> bool| {
        units.iter().any(|unit| {
            unit.function
                .values
                .iter()
                .enumerate()
                .any(|(id, value)| unit.values.needed[id] && predicate(&value.op))
        })
    };
    let mut intrinsic_kinds: BTreeSet<_> = units
        .iter()
        .flat_map(|unit| {
            unit.function
                .values
                .iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    if unit.values.needed[index]
                        && let Op::Intrinsic(kind, _) = value.op
                    {
                        Some(kind)
                    } else {
                        None
                    }
                })
        })
        .collect();
    let global_records = intrinsic_kinds.iter().any(|kind| {
        matches!(
            kind,
            Intrinsic::GlobalGet | Intrinsic::GlobalSet | Intrinsic::GlobalDeclare
        )
    }) || has_op(
        |op| matches!(op,Op::Global(name) if name.scalar_text().is_none_or(|text| !identifier(text))),
    );
    if global_records {
        intrinsic_kinds.insert(Intrinsic::GlobalGet);
    }
    if intrinsic_kinds.contains(&Intrinsic::ObjectRest)
        || intrinsic_kinds.contains(&Intrinsic::OwnNamed)
    {
        intrinsic_kinds.insert(Intrinsic::PropertyKey);
    }
    if intrinsic_kinds.contains(&Intrinsic::AsyncIterator) {
        intrinsic_kinds.insert(Intrinsic::CloseIterator);
    }
    if intrinsic_kinds.contains(&Intrinsic::CloseIterator)
        || intrinsic_kinds.contains(&Intrinsic::AsyncIterator)
    {
        intrinsic_kinds.insert(Intrinsic::IteratorNext);
    }
    statements.extend(super::extra::helpers(&ast, &intrinsic_kinds));
    if intrinsic_kinds.iter().any(|kind| {
        matches!(
            kind,
            Intrinsic::SendableClass
                | Intrinsic::SendableEnv
                | Intrinsic::SendableGet
                | Intrinsic::SendableSet
                | Intrinsic::SendableLoadClass
                | Intrinsic::ConcurrentResult
        )
    }) {
        statements.extend(super::sendable::helpers(&ast));
    }
    if intrinsic_kinds.iter().any(|kind| {
        matches!(
            kind,
            Intrinsic::PrivateGet
                | Intrinsic::PrivateSet
                | Intrinsic::PrivateDefine
                | Intrinsic::PrivateHas
                | Intrinsic::PrivateKeys
        )
    }) {
        statements.extend(super::private::helpers(&ast));
    }
    if uses_standard_iterator
        || has_op(|op| matches!(op, Op::ArraySpread(..)))
        || intrinsic_kinds.contains(&Intrinsic::AsyncIterator)
    {
        statements.push(super::intrinsics::get_iterator(&ast));
    }
    if has_op(|op| matches!(op, Op::ArraySpread(..))) {
        statements.push(super::iteration::array_spread(&ast));
    }
    if has_op(|op| matches!(op, Op::Apply(..))) {
        statements.push(super::iteration::apply(&ast));
    }
    let async_intrinsics = intrinsic_kinds.iter().any(|kind| {
        matches!(
            kind,
            Intrinsic::AsyncEnter
                | Intrinsic::AsyncAwait
                | Intrinsic::AsyncResolve
                | Intrinsic::AsyncReject
                | Intrinsic::AsyncGenReject
                | Intrinsic::AsyncPause
                | Intrinsic::GeneratorState
        )
    });
    if units.iter().any(|unit| unit.function.frame.generator)
        || has_op(|op| matches!(op, Op::GeneratorResume(..)))
        || async_intrinsics
    {
        statements.extend(super::generator::helpers(&ast, false));
    }
    if units.iter().any(|unit| unit.function.frame.async_generator) {
        statements.extend(super::generator::helpers(&ast, true));
    }
    if async_intrinsics {
        statements.extend(super::async_frame::helpers(&ast));
    }
    if units.iter().any(|unit| unit.function.frame.multi_generator) {
        statements.push(super::generator::alias(&ast));
    }
    if has_op(|op| matches!(op, Op::Keys(_))) {
        statements.push(super::iteration::property_iterator(&ast));
    }
    let mut own = false;
    let mut accessors = false;
    for unit in units {
        for (block, events) in unit.function.blocks.iter().enumerate() {
            if !unit.body.owned[block] {
                continue;
            }
            for (index, event) in events.events.iter().enumerate() {
                if !unit.values.keep[block][index] {
                    continue;
                }
                if let Event::Action { action, .. } = event {
                    own |= matches!(action, Action::Set { object, own: StoreKind::ClassSensitive, .. } if !unit.values.ordinary[object.0]);
                    accessors |= matches!(action, Action::Accessors { .. });
                }
            }
        }
    }
    statements.extend(super::classes::helpers(
        &ast,
        has_op(|op| matches!(op, Op::Class { .. })),
        has_op(|op| matches!(op, Op::Method { .. })),
        own || intrinsic_kinds.contains(&Intrinsic::OwnNamed),
        accessors,
    ));
    if units.iter().any(|unit| {
        unit.function
            .values
            .iter()
            .any(|value| matches!(value.op, Op::CopyData(..)))
    }) {
        statements.push(super::intrinsics::copy_data(&ast));
    }
    if main.is_some() {
        statements.extend(super::module::preamble(
            &ast,
            module.ok_or_else(|| invalid("Ark record has no module"))?,
        ));
    }
    for unit in units {
        let frame = &unit.function.frame;
        let written_implicit = [frame.func, frame.new_target, frame.this_reg].map(|register| {
            register.is_some_and(|register| {
                unit.function.blocks.iter().enumerate().any(|(block, events)| {
                    unit.body.owned[block]
                        && events.events.iter().enumerate().any(|(index, event)| {
                            unit.values.keep[block][index]
                                && matches!(event, Event::Action { action: Action::Write(Slot::Register(target), _), .. } if *target == register)
                        })
                })
            })
        });
        let lower = Lower {
            ast: &ast,
            unit,
            written_implicit,
            unverified_version,
            module,
            budget,
            module_mode: main.is_some(),
            global_records,
            warnings: &warnings,
        };
        statements.push(lower.function().map_err(|error| {
            crate::decompile::method_context(unit.function.method.offset, error)
        })?);
        if arena.used_bytes() > budget.metadata_bytes {
            return Err(crate::decompile::method_context(
                unit.function.method.offset,
                resource("Ark AST exceeds metadata admission"),
            ));
        }
    }
    if let Some(main) = main {
        statements.extend(super::module::finish(
            &ast,
            module.ok_or_else(|| invalid("Ark record has no module"))?,
            main,
        )?);
    }
    let warnings = warnings.into_inner();
    let program = Program::new(
        SPAN,
        SourceType::mjs(),
        &warnings.source,
        ast.vec(warnings.comments),
        None,
        ast.vec([]),
        ast.vec(statements),
        &ast.build,
    );
    let code = Codegen::new()
        .with_options(CodegenOptions::default())
        .build(&program)
        .code;
    let mut output = Output::new(budget.output_bytes);
    output.write_all(code.as_bytes())?;
    Ok((output.into_bytes(), warnings.kinds))
}

pub(super) fn string_literal<'a>(ast: &Ast<'a>, name: &crate::Name) -> StringLiteral<'a> {
    if let Expression::StringLiteral(value) = ast.name(name) {
        value.unbox()
    } else {
        unreachable!()
    }
}

fn imports<'a>(
    ast: &Ast<'a>,
    units: &[Prepared<'_>],
    module: Option<&Module>,
) -> Result<Vec<Statement<'a>>> {
    let mut imports = BTreeSet::new();
    let mut namespaces = BTreeSet::new();
    for unit in units {
        for (index, value) in unit.function.values.iter().enumerate() {
            if !unit.values.needed[index] {
                continue;
            }
            match value.op {
                Op::Import(index) => {
                    imports.insert(index);
                }
                Op::Namespace(index) => {
                    namespaces.insert(index);
                }
                _ => {}
            }
        }
    }
    let mut statements = Vec::new();
    for index in imports {
        let module = module.ok_or_else(|| invalid("Ark import lacks module metadata"))?;
        let regular = module.regular_imports.get(index as usize);
        let import = regular
            .or_else(|| {
                module
                    .namespace_imports
                    .get(index as usize - module.regular_imports.len())
            })
            .ok_or_else(|| invalid("Ark import index exceeds metadata"))?;
        let local = ast.binding(&format!("__ark_i_{index}"));
        let specifier = match &import.import_name {
            None => {
                ImportDeclarationSpecifier::new_import_namespace_specifier(SPAN, local, &ast.build)
            }
            Some(name) if name.text == "default" => {
                ImportDeclarationSpecifier::new_import_default_specifier(SPAN, local, &ast.build)
            }
            Some(name) => ImportDeclarationSpecifier::new_import_specifier(
                SPAN,
                ModuleExportName::StringLiteral(string_literal(ast, name)),
                local,
                ImportOrExportKind::Value,
                &ast.build,
            ),
        };
        let request = module
            .requests
            .get(import.request as usize)
            .ok_or_else(|| invalid("Ark import request exceeds metadata"))?;
        statements.push(Statement::new_import_declaration(
            SPAN,
            Some(ast.vec([specifier])),
            string_literal(ast, request),
            None,
            None,
            ImportOrExportKind::Value,
            &ast.build,
        ));
    }
    for index in namespaces {
        let module = module.ok_or_else(|| invalid("Ark namespace lacks module metadata"))?;
        let request = module
            .requests
            .get(index as usize)
            .ok_or_else(|| invalid("Ark namespace request exceeds metadata"))?;
        let specifier = ImportDeclarationSpecifier::new_import_namespace_specifier(
            SPAN,
            ast.binding(&format!("__ark_ns_{index}")),
            &ast.build,
        );
        statements.push(Statement::new_import_declaration(
            SPAN,
            Some(ast.vec([specifier])),
            string_literal(ast, request),
            None,
            None,
            ImportOrExportKind::Value,
            &ast.build,
        ));
    }
    Ok(statements)
}

struct Lower<'f, 'a, 's> {
    ast: &'f Ast<'a>,
    unit: &'f Prepared<'s>,
    written_implicit: [bool; 3],
    unverified_version: bool,
    module: Option<&'f Module>,
    budget: Budget,
    module_mode: bool,
    global_records: bool,
    warnings: &'f RefCell<SourceWarnings>,
}

impl<'a> Lower<'_, 'a, '_> {
    fn function(&self) -> Result<Statement<'a>> {
        let function = &self.unit.function;
        let mut slots = BTreeSet::new();
        for (index, value) in function.values.iter().enumerate() {
            if self.unit.values.needed[index]
                && let Op::Read(slot) = value.op
            {
                slots.insert(slot_number(slot));
            }
        }
        for (index, block) in function.blocks.iter().enumerate() {
            if !self.unit.body.owned[index] {
                continue;
            }
            for (position, event) in block.events.iter().enumerate() {
                if self.unit.values.keep[index][position]
                    && let Event::Action {
                        action: Action::Write(slot, _),
                        ..
                    } = event
                {
                    slots.insert(slot_number(*slot));
                }
            }
            if !block.handlers.is_empty() {
                slots.insert(0);
            }
        }
        let mut body = Vec::new();
        if function.frame.constructor {
            body.push(self.ast.var(
                "__ark_this",
                Some(self.ast.property(self.ast.id("__ark_context"), "receiver")),
                false,
            ));
        }
        for (index, register) in [
            function.frame.func,
            function.frame.new_target,
            function.frame.this_reg,
        ]
        .into_iter()
        .enumerate()
        {
            if self.written_implicit[index] {
                let register = register.ok_or_else(|| invalid("missing Ark implicit register"))?;
                let initial = match index {
                    0 => self.current_function(),
                    1 => self.initial_new_target(),
                    _ => self.this(),
                };
                body.push(
                    self.ast
                        .var(&format!("__ark_reg_{register}"), Some(initial), false),
                );
            }
        }
        let uses_env = function.values.iter().any(|value| {
            matches!(
                value.op,
                Op::LoadCell { .. }
                    | Op::Environment
                    | Op::Closure { .. }
                    | Op::Class { .. }
                    | Op::Method { .. }
            )
        }) || function
            .blocks
            .iter()
            .flat_map(|block| &block.events)
            .any(|event| {
                matches!(
                    event,
                    Event::Action {
                        action: Action::StoreCell { .. } | Action::PushEnv(_) | Action::PopEnv,
                        ..
                    }
                )
            });
        if uses_env {
            body.push(self.ast.var(
                "__ark_env",
                Some(self.ast.property(self.ast.id("__ark_context"), "env")),
                false,
            ));
        }
        for number in slots {
            if number == 0 {
                body.push(self.ast.var("acc", None, false));
                continue;
            }
            if number == 1 {
                body.push(self.ast.var("__ark_resume", None, false));
                continue;
            }
            let reg = number - 2;
            if reg < function.frame.locals {
                body.push(self.ast.var(&format!("v{reg}"), None, false));
            }
        }
        body.extend(self.nodes(&self.unit.body.nodes)?);
        let function_name = format!("__ark_f_{:x}", function.method.offset);
        let params: Vec<_> = (0..function.frame.params)
            .map(|index| format!("p{index}"))
            .collect();
        let arrow_arguments = function.frame.arrow
            && function.values.iter().any(|value| {
                matches!(
                    value.op,
                    Op::RestArgs(_) | Op::Intrinsic(Intrinsic::Arguments, _)
                )
            });
        let expression = if function.frame.generator {
            let mut inner = self.ast.function(
                "__ark_run",
                params.clone(),
                body,
                function.frame.async_generator,
            );
            if let Expression::FunctionExpression(inner) = &mut inner {
                inner.generator = true;
            }
            let run = self.ast.call(
                self.ast.property(self.ast.id("Reflect"), "apply"),
                vec![inner, self.this(), self.ast.id("arguments")],
            );
            let wrapper = vec![
                self.ast.if_(
                    self.ast.binary(
                        Expression::new_new_target(SPAN, &self.ast.build),
                        B::StrictInequality,
                        self.ast.undefined(),
                    ),
                    vec![self.ast.throw(self.ast.construct(
                        self.ast.id("TypeError"),
                        vec![self.ast.string("Generator is not a constructor")],
                    ))],
                    None,
                ),
                self.ast
                    .var("__ark_generator", Some(self.ast.object(vec![])), true),
                self.ast.ret(Some(self.ast.call(
                    self.ast.id(if function.frame.async_generator {
                        "__ark_async_drive"
                    } else {
                        "__ark_drive"
                    }),
                    vec![run, self.ast.id("__ark_generator")],
                ))),
            ];
            self.ast.function(&function_name, params, wrapper, false)
        } else if function.frame.arrow && !arrow_arguments {
            let body = FunctionBody::boxed(
                SPAN,
                self.ast.vec([Directive::new_use_strict(&self.ast.build)]),
                self.ast.vec(body),
                &self.ast.build,
            );
            Expression::new_arrow_function_expression(
                SPAN,
                function.frame.asynchronous,
                None,
                self.ast.parameters(params),
                None,
                ArrowFunctionBody::FunctionBody(body),
                &self.ast.build,
            )
        } else {
            self.ast
                .function(&function_name, params, body, function.frame.asynchronous)
        };
        let factory_body = vec![
            self.ast.var(&function_name, Some(expression), true),
            self.ast.ret(Some(self.ast.id(&function_name))),
        ];
        let mut declaration = self.ast.factory(
            &format!("_ark_capture_{:x}", function.method.offset),
            vec!["__ark_context".into()],
            factory_body,
        );
        if self.unverified_version {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::VersionSemantics,
            )?;
        }
        if function.frame.constructor {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::ClassReconstruction,
            )?;
        }
        if self.global_records {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::RuntimeBindings,
            )?;
        }
        if arrow_arguments {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::JavaScriptIntrinsics,
            )?;
        }
        if function.frame.raw_async || function.frame.async_generator {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::AsyncFrameApproximation,
            )?;
        }
        if function.frame.typed_catches {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::ExceptionTypeSemantics,
            )?;
        }
        if function.frame.multi_generator {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::CoroutineAliasing,
            )?;
        }
        if function.frame.frame_warning {
            self.warnings.borrow_mut().attach(
                &mut declaration,
                crate::ReconstructionWarning::FrameConventions,
            )?;
        }
        Ok(declaration)
    }

    fn slot(&self, slot: Slot) -> Expression<'a> {
        let frame = &self.unit.function.frame;
        match slot {
            Slot::Acc => self.ast.id("acc"),
            Slot::Resume => self.ast.id("__ark_resume"),
            Slot::Register(reg)
                if [frame.func, frame.new_target, frame.this_reg]
                    .into_iter()
                    .enumerate()
                    .any(|(index, implicit)| {
                        implicit == Some(reg) && self.written_implicit[index]
                    }) =>
            {
                self.ast.id(&format!("__ark_reg_{reg}"))
            }
            Slot::Register(reg) if Some(reg) == frame.func => self.current_function(),
            Slot::Register(reg) if Some(reg) == frame.this_reg => self.this(),
            Slot::Register(reg) if Some(reg) == frame.new_target => self.new_target(),
            Slot::Register(reg) if reg >= frame.first_param => {
                self.ast.id(&format!("p{}", reg - frame.first_param))
            }
            Slot::Register(reg) => self.ast.id(&format!("v{reg}")),
        }
    }
    fn current_function(&self) -> Expression<'a> {
        if self.unit.function.frame.constructor {
            self.ast.property(self.ast.id("__ark_context"), "function")
        } else {
            self.ast
                .id(&format!("__ark_f_{:x}", self.unit.function.method.offset))
        }
    }
    fn this(&self) -> Expression<'a> {
        if self.unit.function.frame.constructor {
            self.ast.id("__ark_this")
        } else if self.unit.function.frame.arrow {
            self.ast.property(self.ast.id("__ark_context"), "receiver")
        } else {
            Expression::new_this_expression(SPAN, &self.ast.build)
        }
    }
    fn new_target(&self) -> Expression<'a> {
        if self.written_implicit[1]
            && let Some(reg) = self.unit.function.frame.new_target
        {
            return self.ast.id(&format!("__ark_reg_{reg}"));
        }
        self.initial_new_target()
    }
    fn initial_new_target(&self) -> Expression<'a> {
        if self.unit.function.frame.arrow || self.unit.function.frame.constructor {
            self.ast.property(self.ast.id("__ark_context"), "newTarget")
        } else {
            Expression::new_new_target(SPAN, &self.ast.build)
        }
    }
    fn context(&self) -> Expression<'a> {
        self.context_env(self.ast.id("__ark_env"))
    }
    fn context_env(&self, env: Expression<'a>) -> Expression<'a> {
        self.ast.object(vec![
            (
                self.ast.string("home"),
                self.ast.property(self.ast.id("__ark_context"), "home"),
            ),
            (self.ast.string("env"), env),
            (
                self.ast.string("hole"),
                self.ast.property(self.ast.id("__ark_context"), "hole"),
            ),
            (self.ast.string("receiver"), self.this()),
            (self.ast.string("newTarget"), self.new_target()),
            (
                self.ast.string("locals"),
                self.ast.property(self.ast.id("__ark_context"), "locals"),
            ),
        ])
    }
    fn cell(&self, depth: u32, index: u32) -> Result<Expression<'a>> {
        if depth as usize > self.budget.ast_depth {
            return Err(resource("Ark lexical access exceeds depth guard"));
        }
        let mut env = self.ast.id("__ark_env");
        for _ in 0..depth {
            env = self.ast.property(env, "parent");
        }
        Ok(self.ast.member(
            self.ast.property(env, "slots"),
            self.ast.number(f64::from(index)),
        ))
    }
    fn module(&self, index: u32) -> Result<Expression<'a>> {
        let module = self
            .module
            .ok_or_else(|| invalid("Ark local binding lacks module metadata"))?;
        let export = module
            .local_exports
            .get(index as usize)
            .ok_or_else(|| invalid("Ark local export exceeds metadata"))?;
        if self.module_mode {
            let index = super::module::local_index(
                self.module
                    .ok_or_else(|| invalid("Ark module is missing"))?,
                index as usize,
            );
            return Ok(self.ast.id(&format!("__ark_local_{index}")));
        }
        Ok(self.ast.member(
            self.ast.property(self.ast.id("__ark_context"), "locals"),
            self.ast.name(&export.local_name),
        ))
    }
    fn key(&self, key: &Key, depth: usize) -> Result<Expression<'a>> {
        match key {
            Key::Name(name) => Ok(self.ast.name(name)),
            Key::Index(value) => Ok(self.ast.number(f64::from(*value))),
            Key::Value(value) => self.value(*value, depth + 1),
        }
    }
    fn value(&self, id: ValueId, depth: usize) -> Result<Expression<'a>> {
        if self.unit.values.materialize[id.0] {
            return Ok(self.ast.id(&format!("t{}", id.0)));
        }
        self.definition(id, depth)
    }
    fn definition(&self, id: ValueId, depth: usize) -> Result<Expression<'a>> {
        if depth > self.budget.ast_depth / 2 {
            return Err(resource("Ark expression nesting exceeds depth guard"));
        }
        let value = &self.unit.function.values[id.0];
        let result = match &value.op {
            Op::Constant(constant) => match constant {
                Constant::Undefined => self.ast.undefined(),
                Constant::Null => Expression::new_null_literal(SPAN, &self.ast.build),
                Constant::Bool(value) => self.ast.boolean(*value),
                Constant::Number(value) => self.ast.number(*value),
                Constant::String(name) => self.ast.name(name),
                Constant::Hole => self.ast.property(self.ast.id("__ark_context"), "hole"),
            },
            Op::Read(slot) => self.slot(*slot),
            Op::This => self.this(),
            Op::NewTarget => self.new_target(),
            Op::Function => self.current_function(),
            Op::Global(name) => {
                if !self.global_records
                    && let Some(text) = name.scalar_text().filter(|text| identifier(text))
                {
                    self.ast.id(text)
                } else {
                    self.ast
                        .call(self.ast.id("__ark_global_get"), vec![self.ast.name(name)])
                }
            }
            Op::GlobalObject => self.ast.id("globalThis"),
            Op::Unary(op, input) => self.ast.unary(*op, self.value(*input, depth + 1)?),
            Op::Binary(op, left, right) | Op::BranchCompare(op, left, right) => self.ast.binary(
                self.value(*left, depth + 1)?,
                *op,
                self.value(*right, depth + 1)?,
            ),
            Op::Get(object, key) => self
                .ast
                .member(self.value(*object, depth + 1)?, self.key(key, depth + 1)?),
            Op::Call {
                callee,
                receiver,
                args,
            } => {
                let callee = self.value(*callee, depth + 1)?;
                let args = args
                    .iter()
                    .map(|value| self.value(*value, depth + 1))
                    .collect::<Result<Vec<_>>>()?;
                if let Some(receiver) = receiver {
                    self.ast.call(
                        self.ast.property(self.ast.id("Reflect"), "apply"),
                        vec![
                            callee,
                            self.value(*receiver, depth + 1)?,
                            self.ast.array(args),
                        ],
                    )
                } else {
                    let indirect = Expression::new_sequence_expression(
                        SPAN,
                        self.ast.vec([self.ast.number(0.0), callee]),
                        &self.ast.build,
                    );
                    self.ast.call(indirect, args)
                }
            }
            Op::Construct { callee, args } => self.ast.construct(
                self.value(*callee, depth + 1)?,
                args.iter()
                    .map(|value| self.value(*value, depth + 1))
                    .collect::<Result<Vec<_>>>()?,
            ),
            Op::Array(values) => self.ast.array(
                values
                    .iter()
                    .map(|value| self.value(*value, depth + 1))
                    .collect::<Result<Vec<_>>>()?,
            ),
            Op::Object(entries) => self.ast.object(
                entries
                    .iter()
                    .map(|(key, value)| {
                        Ok((self.key(key, depth + 1)?, self.value(*value, depth + 1)?))
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
            Op::LoadCell {
                depth: parent,
                index,
            } => self.cell(*parent, *index)?,
            Op::Environment => self.ast.id("__ark_env"),
            Op::Home => self.ast.property(self.ast.id("__ark_context"), "home"),
            Op::Import(index) => self.ast.id(&format!("__ark_i_{index}")),
            Op::Namespace(index) => self.ast.id(&format!("__ark_ns_{index}")),
            Op::Module(index) => self.module(*index)?,
            Op::Closure {
                method,
                length,
                environment,
            } => {
                let context = if let Some(environment) = environment {
                    self.context_env(self.value(*environment, depth + 1)?)
                } else {
                    self.context()
                };
                let call = self.ast.call(
                    self.ast.id(&format!("_ark_capture_{method:x}")),
                    vec![context],
                );
                if let Some(length) = length {
                    self.ast.call(
                        self.ast.property(self.ast.id("Object"), "defineProperty"),
                        vec![
                            call,
                            self.ast.string("length"),
                            self.ast.object(vec![(
                                self.ast.string("value"),
                                self.ast.number(f64::from(*length)),
                            )]),
                        ],
                    )
                } else {
                    call
                }
            }
            Op::Class {
                constructor,
                name,
                base,
                environment,
                length,
                members,
            } => {
                let members = members
                    .iter()
                    .map(|member| {
                        let value = if let Some(method) = member.method {
                            self.ast.id(&format!("_ark_capture_{method:x}"))
                        } else {
                            self.value(
                                member
                                    .value
                                    .ok_or_else(|| invalid("class member lacks a value"))?,
                                depth + 1,
                            )?
                        };
                        Ok(self.ast.array(vec![
                            self.key(&member.name, depth + 1)?,
                            value,
                            self.ast.number(f64::from(member.length)),
                            self.ast.boolean(member.is_static),
                            self.ast.boolean(member.method.is_some()),
                            self.ast.boolean(member.accessor),
                        ]))
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.ast.call(
                    self.ast.id("__ark_class"),
                    vec![
                        self.ast.id(&format!("_ark_capture_{constructor:x}")),
                        self.value(*base, depth + 1)?,
                        self.ast.number(f64::from(*length)),
                        self.ast.array(members),
                        if let Some(environment) = environment {
                            self.context_env(self.value(*environment, depth + 1)?)
                        } else {
                            self.context()
                        },
                        self.ast.method_name(name),
                    ],
                )
            }
            Op::Method {
                method,
                name,
                length,
                home,
            } => self.ast.call(
                self.ast.id("__ark_method"),
                vec![
                    self.ast.id(&format!("_ark_capture_{method:x}")),
                    self.ast.number(f64::from(*length)),
                    self.value(*home, depth + 1)?,
                    self.context(),
                    self.ast.method_name(name),
                ],
            ),
            Op::SuperCall {
                function,
                args,
                spread,
            } => {
                let function = function
                    .map(|id| self.value(id, depth + 1))
                    .transpose()?
                    .unwrap_or_else(|| self.current_function());
                let args = if let Some(spread) = spread {
                    Expression::new_array_expression(
                        SPAN,
                        self.ast.vec([ArrayExpressionElement::new_spread_element(
                            SPAN,
                            self.value(*spread, depth + 1)?,
                            &self.ast.build,
                        )]),
                        &self.ast.build,
                    )
                } else {
                    self.ast.array(
                        args.iter()
                            .map(|id| self.value(*id, depth + 1))
                            .collect::<Result<Vec<_>>>()?,
                    )
                };
                self.ast.call(
                    self.ast.property(self.ast.id("Reflect"), "construct"),
                    vec![
                        self.ast.call(
                            self.ast.property(self.ast.id("Object"), "getPrototypeOf"),
                            vec![function],
                        ),
                        args,
                        self.new_target(),
                    ],
                )
            }
            Op::SuperGet(receiver, key) => self.ast.call(
                self.ast.property(self.ast.id("Reflect"), "get"),
                vec![
                    self.ast.call(
                        self.ast.property(self.ast.id("Object"), "getPrototypeOf"),
                        vec![self.ast.property(self.ast.id("__ark_context"), "home")],
                    ),
                    self.key(key, depth + 1)?,
                    self.value(*receiver, depth + 1)?,
                ],
            ),
            Op::Await(input) => Expression::new_await_expression(
                SPAN,
                self.value(*input, depth + 1)?,
                &self.ast.build,
            ),
            Op::ToNumeric(input) | Op::Update(input, _) => {
                // Postfix update returns ToNumeric's old value, including boxed BigInt, after one coercion.
                let (increment, prefix) = match value.op {
                    Op::Update(_, increment) => (increment, true),
                    _ => (true, false),
                };
                let target =
                    SimpleAssignmentTarget::AssignmentTargetIdentifier(IdentifierReference::boxed(
                        SPAN,
                        Ident::from_str_in("__ark_u", &self.ast.build),
                        &self.ast.build,
                    ));
                let update = Expression::new_update_expression(
                    SPAN,
                    if increment {
                        UpdateOperator::Increment
                    } else {
                        UpdateOperator::Decrement
                    },
                    prefix,
                    target,
                    &self.ast.build,
                );
                let function = self.ast.function(
                    "__ark_update",
                    vec!["__ark_u".into()],
                    vec![self.ast.ret(Some(update))],
                    false,
                );
                self.ast
                    .call(function, vec![self.value(*input, depth + 1)?])
            }
            Op::Intrinsic(kind, values) => {
                let args = values
                    .iter()
                    .map(|value| self.value(*value, depth + 1))
                    .collect::<Result<Vec<_>>>()?;
                match kind {
                    Intrinsic::AsyncAwait => self.ast.property(
                        Expression::new_await_expression(
                            SPAN,
                            self.ast.call(self.ast.id("__ark_async_await"), args),
                            &self.ast.build,
                        ),
                        "promise",
                    ),
                    Intrinsic::Accessor => self.ast.object(vec![
                        (self.ast.string("get"), self.ast.undefined()),
                        (self.ast.string("set"), self.ast.undefined()),
                    ]),
                    Intrinsic::SendableImport
                    | Intrinsic::SendableLocal
                    | Intrinsic::LazyImport => args
                        .into_iter()
                        .next()
                        .ok_or_else(|| invalid("missing import binding"))?,
                    Intrinsic::BigInt => self.ast.call(self.ast.id("BigInt"), args),
                    Intrinsic::Symbol => self.ast.id("Symbol"),
                    Intrinsic::Arguments => self.ast.id("arguments"),
                    Intrinsic::ConstructSpread => {
                        let mut args = args.into_iter();
                        let constructor = args
                            .next()
                            .ok_or_else(|| invalid("missing spread constructor"))?;
                        let values = args
                            .next()
                            .ok_or_else(|| invalid("missing spread arguments"))?;
                        let values = Expression::new_array_expression(
                            SPAN,
                            self.ast.vec([ArrayExpressionElement::new_spread_element(
                                SPAN,
                                values,
                                &self.ast.build,
                            )]),
                            &self.ast.build,
                        );
                        self.ast.call(
                            self.ast.property(self.ast.id("Reflect"), "construct"),
                            vec![constructor, values],
                        )
                    }
                    _ => self
                        .ast
                        .call(self.ast.id(&format!("__ark_{}", kind.name())), args),
                }
            }
            Op::NamedError(kind, prefix, value) => {
                let message = if let Some(value) = value {
                    self.ast.binary(
                        self.ast.string(prefix),
                        B::Addition,
                        self.value(*value, depth + 1)?,
                    )
                } else {
                    self.ast.string(prefix)
                };
                self.ast.construct(self.ast.id(kind), vec![message])
            }
            Op::TypeError(message) => self
                .ast
                .construct(self.ast.id("TypeError"), vec![self.ast.string(message)]),
            Op::Generator => {
                if self.unit.function.frame.multi_generator {
                    self.ast.call(
                        self.ast.id("__ark_generator_alias"),
                        vec![self.ast.id("__ark_generator")],
                    )
                } else {
                    self.ast.id("__ark_generator")
                }
            }
            Op::Suspend(generator, value) => {
                let result = self.ast.call(
                    self.ast.id("__ark_suspend"),
                    vec![
                        self.ast.id("__ark_generator"),
                        self.value(*generator, depth + 1)?,
                        self.value(*value, depth + 1)?,
                    ],
                );
                let result = if self.unit.function.frame.async_generator {
                    self.ast.object(vec![
                        (self.ast.string("__ark_internal"), self.ast.boolean(true)),
                        (self.ast.string("value"), result),
                    ])
                } else {
                    result
                };
                Expression::new_yield_expression(SPAN, false, Some(result), &self.ast.build)
            }
            Op::AsyncYield(generator, value, done) => {
                let payload = self.ast.object(vec![
                    (self.ast.string("value"), self.value(*value, depth + 1)?),
                    (self.ast.string("done"), self.value(*done, depth + 1)?),
                ]);
                let payload = self.ast.call(
                    self.ast.id("__ark_suspend"),
                    vec![
                        self.ast.id("__ark_generator"),
                        self.value(*generator, depth + 1)?,
                        payload,
                    ],
                );
                Expression::new_yield_expression(SPAN, false, Some(payload), &self.ast.build)
            }
            Op::GeneratorResume(generator, mode) => self.ast.property(
                self.ast.call(
                    self.ast
                        .property(self.ast.id("__ark_generator_state"), "get"),
                    vec![self.value(*generator, depth + 1)?],
                ),
                if *mode { "mode" } else { "value" },
            ),
            Op::IterResult(value, done) => self.ast.object(vec![
                (self.ast.string("value"), self.value(*value, depth + 1)?),
                (self.ast.string("done"), self.value(*done, depth + 1)?),
            ]),
            Op::Apply(function, receiver, args) => self.ast.call(
                self.ast.id("__ark_apply"),
                vec![
                    self.value(*function, depth + 1)?,
                    self.value(*receiver, depth + 1)?,
                    self.value(*args, depth + 1)?,
                ],
            ),
            Op::Iterator(input) => self.ast.call(
                self.ast.id("__ark_get_iterator"),
                vec![self.value(*input, depth + 1)?],
            ),
            Op::Keys(input) => self.ast.call(
                self.ast.id("__ark_property_iterator"),
                vec![self.value(*input, depth + 1)?],
            ),
            Op::Next(input) => self.ast.property(
                self.ast.call(
                    self.ast.property(self.value(*input, depth + 1)?, "next"),
                    vec![],
                ),
                "value",
            ),
            Op::ArraySpread(dst, index, src) => self.ast.call(
                self.ast.id("__ark_array_spread"),
                vec![
                    self.value(*dst, depth + 1)?,
                    self.value(*index, depth + 1)?,
                    self.value(*src, depth + 1)?,
                ],
            ),
            Op::DynamicImport(input) => Expression::new_import_expression(
                SPAN,
                self.value(*input, depth + 1)?,
                None,
                None,
                &self.ast.build,
            ),
            Op::Delete(object, key) => self.ast.unary(
                U::Delete,
                self.ast.member(
                    self.value(*object, depth + 1)?,
                    self.value(*key, depth + 1)?,
                ),
            ),
            Op::CopyData(target, source) => self.ast.call(
                self.ast.id("__ark_copy"),
                vec![
                    self.value(*target, depth + 1)?,
                    self.value(*source, depth + 1)?,
                ],
            ),
            Op::RestArgs(start) => {
                let slice = self.ast.property(
                    self.ast.property(self.ast.id("Array"), "prototype"),
                    "slice",
                );
                self.ast.call(
                    self.ast.property(slice, "call"),
                    vec![self.ast.id("arguments"), self.ast.number(f64::from(*start))],
                )
            }
        };
        let mut result = result;
        *result.span_mut() = Span::new(value.pc, value.pc + 1);
        Ok(result)
    }
    fn events(&self, block: usize) -> Result<Vec<Statement<'a>>> {
        let mut statements = Vec::new();
        for (index, event) in self.unit.function.blocks[block].events.iter().enumerate() {
            match event {
                Event::Define(id) if self.unit.values.needed[id.0] => {
                    let value = &self.unit.function.values[id.0];
                    let statement = if self.unit.values.materialize[id.0] {
                        Some(self.ast.var(
                            &format!("t{}", id.0),
                            Some(self.definition(*id, 0)?),
                            true,
                        ))
                    } else if value.effects.ordered() && self.unit.values.uses[id.0] == 0 {
                        Some(self.ast.expr(self.definition(*id, 0)?))
                    } else {
                        None
                    };
                    if let Some(mut statement) = statement {
                        if let Some(warning) = value.op.warning() {
                            self.warnings.borrow_mut().attach(&mut statement, warning)?;
                        }
                        statements.push(statement);
                    }
                }
                Event::Action { pc, action } if self.unit.values.keep[block][index] => {
                    let warned = matches!(
                        action,
                        Action::Accessors { .. } | Action::SuperSet { .. }
                    ) || matches!(action, Action::Set { object, own: StoreKind::ClassSensitive, .. } if !self.unit.values.ordinary[object.0]);
                    let mut action = self.action(action)?;
                    *action.span_mut() = Span::new(*pc, *pc + 1);
                    if warned {
                        self.warnings.borrow_mut().attach(
                            &mut action,
                            crate::ReconstructionWarning::ClassReconstruction,
                        )?;
                    }
                    statements.push(action);
                }
                _ => {}
            }
        }
        Ok(statements)
    }
    fn action(&self, action: &Action) -> Result<Statement<'a>> {
        Ok(match action {
            Action::Write(slot, value) => self.ast.assign(self.slot(*slot), self.value(*value, 0)?),
            Action::Debugger => Statement::new_debugger_statement(SPAN, &self.ast.build),
            Action::CheckHoleValue(value, name) => self.ast.if_(
                self.ast.binary(
                    self.value(*value, 0)?,
                    B::StrictEquality,
                    self.ast.property(self.ast.id("__ark_context"), "hole"),
                ),
                vec![self.ast.throw(self.ast.construct(
                    self.ast.id("ReferenceError"),
                    vec![self.ast.binary(
                        self.value(*name, 0)?,
                        B::Addition,
                        self.ast.string(" is not initialized"),
                    )],
                ))],
                None,
            ),
            Action::CheckSuper(value, already) => {
                let empty = Expression::new_logical_expression(
                    SPAN,
                    self.ast.binary(
                        self.value(*value, 0)?,
                        B::StrictEquality,
                        self.ast.undefined(),
                    ),
                    L::Or,
                    self.ast.binary(
                        self.value(*value, 0)?,
                        B::StrictEquality,
                        self.ast.property(self.ast.id("__ark_context"), "hole"),
                    ),
                    &self.ast.build,
                );
                let test = if *already {
                    self.ast.unary(U::LogicalNot, empty)
                } else {
                    empty
                };
                self.ast.if_(
                    test,
                    vec![self.ast.throw(self.ast.construct(
                        self.ast.id("ReferenceError"),
                        vec![self.ast.string(if *already {
                            "super() forbidden re-bind 'this'"
                        } else {
                            "sub-class must call super before use 'this'"
                        })],
                    ))],
                    None,
                )
            }
            Action::Accessors {
                object,
                key,
                getter,
                setter,
                set_name,
            } => self.ast.expr(self.ast.call(
                self.ast.id("__ark_accessors"),
                vec![
                    self.value(*object, 0)?,
                    self.value(*key, 0)?,
                    self.value(*getter, 0)?,
                    self.value(*setter, 0)?,
                    self.value(*set_name, 0)?,
                ],
            )),
            Action::SuperSet {
                receiver,
                key,
                value,
            } => {
                let set = self.ast.call(
                    self.ast.property(self.ast.id("Reflect"), "set"),
                    vec![
                        self.ast.call(
                            self.ast.property(self.ast.id("Object"), "getPrototypeOf"),
                            vec![self.ast.property(self.ast.id("__ark_context"), "home")],
                        ),
                        self.key(key, 0)?,
                        self.value(*value, 0)?,
                        self.value(*receiver, 0)?,
                    ],
                );
                self.ast.if_(
                    self.ast.unary(U::LogicalNot, set),
                    vec![self.ast.throw(self.ast.construct(
                        self.ast.id("TypeError"),
                        vec![self.ast.string("Cannot assign super property")],
                    ))],
                    None,
                )
            }
            Action::Set {
                object,
                key,
                value,
                own,
            } => {
                if *own != StoreKind::Assign {
                    if *own == StoreKind::ClassSensitive && !self.unit.values.ordinary[object.0] {
                        return Ok(self.ast.expr(self.ast.call(
                            self.ast.id("__ark_define_own"),
                            vec![
                                self.value(*object, 0)?,
                                self.key(key, 0)?,
                                self.value(*value, 0)?,
                            ],
                        )));
                    }
                    let descriptor = self.ast.object(vec![
                        (self.ast.string("value"), self.value(*value, 0)?),
                        (self.ast.string("writable"), self.ast.boolean(true)),
                        (self.ast.string("enumerable"), self.ast.boolean(true)),
                        (self.ast.string("configurable"), self.ast.boolean(true)),
                    ]);
                    self.ast.expr(self.ast.call(
                        self.ast.property(self.ast.id("Object"), "defineProperty"),
                        vec![self.value(*object, 0)?, self.key(key, 0)?, descriptor],
                    ))
                } else {
                    self.ast.assign(
                        self.ast.member(self.value(*object, 0)?, self.key(key, 0)?),
                        self.value(*value, 0)?,
                    )
                }
            }
            Action::StoreCell {
                depth,
                index,
                value,
            } => self
                .ast
                .assign(self.cell(*depth, *index)?, self.value(*value, 0)?),
            Action::StoreModule(index, value) => self
                .ast
                .assign(self.module(*index)?, self.value(*value, 0)?),
            Action::PushEnv(size) => {
                let slots = self.ast.call(
                    self.ast.property(
                        self.ast.call(
                            self.ast.id("Array"),
                            vec![self.ast.number(f64::from(*size))],
                        ),
                        "fill",
                    ),
                    vec![self.ast.property(self.ast.id("__ark_context"), "hole")],
                );
                self.ast.assign(
                    self.ast.id("__ark_env"),
                    self.ast.object(vec![
                        (self.ast.string("parent"), self.ast.id("__ark_env")),
                        (self.ast.string("slots"), slots),
                    ]),
                )
            }
            Action::PopEnv => self.ast.assign(
                self.ast.id("__ark_env"),
                self.ast.property(self.ast.id("__ark_env"), "parent"),
            ),
            Action::CheckHole(value, name) => {
                let test = self.ast.binary(
                    self.value(*value, 0)?,
                    B::StrictEquality,
                    self.ast.property(self.ast.id("__ark_context"), "hole"),
                );
                self.ast.if_(
                    test,
                    vec![self.ast.throw(self.ast.construct(
                        self.ast.id("ReferenceError"),
                        vec![self.ast.binary(
                            self.ast.name(name),
                            B::Addition,
                            self.ast.string(" is not initialized"),
                        )],
                    ))],
                    None,
                )
            }
            Action::CheckObject(value) => {
                let null = self.ast.binary(
                    self.value(*value, 0)?,
                    B::StrictEquality,
                    Expression::new_null_literal(SPAN, &self.ast.build),
                );
                let is_not_object = self.ast.binary(
                    self.ast.unary(U::Typeof, self.value(*value, 0)?),
                    B::StrictInequality,
                    self.ast.string("object"),
                );
                let is_not_function = self.ast.binary(
                    self.ast.unary(U::Typeof, self.value(*value, 0)?),
                    B::StrictInequality,
                    self.ast.string("function"),
                );
                let other = Expression::new_logical_expression(
                    SPAN,
                    is_not_object,
                    L::And,
                    is_not_function,
                    &self.ast.build,
                );
                let test =
                    Expression::new_logical_expression(SPAN, null, L::Or, other, &self.ast.build);
                self.ast.if_(
                    test,
                    vec![self.ast.throw(self.ast.construct(
                        self.ast.id("TypeError"),
                        vec![self.ast.string("Inner return result is not object")],
                    ))],
                    None,
                )
            }
        })
    }
    fn test(&self, value: ValueId, nonzero: bool) -> Result<Expression<'a>> {
        if self.unit.values.boolean[value.0] {
            let value = self.value(value, 0)?;
            return Ok(if nonzero {
                value
            } else {
                self.ast.unary(U::LogicalNot, value)
            });
        }
        let boolean = self.ast.binary(
            self.value(value, 0)?,
            B::StrictEquality,
            self.ast.boolean(nonzero),
        );
        let number = if nonzero {
            let numeric = self.ast.binary(
                self.ast.unary(U::Typeof, self.value(value, 0)?),
                B::StrictEquality,
                self.ast.string("number"),
            );
            let nonzero = self.ast.binary(
                self.value(value, 0)?,
                B::StrictInequality,
                self.ast.number(0.0),
            );
            Expression::new_logical_expression(SPAN, numeric, L::And, nonzero, &self.ast.build)
        } else {
            self.ast.binary(
                self.value(value, 0)?,
                B::StrictEquality,
                self.ast.number(0.0),
            )
        };
        Ok(Expression::new_logical_expression(
            SPAN,
            boolean,
            L::Or,
            number,
            &self.ast.build,
        ))
    }
    fn truthy(&self, expression: Expression<'a>) -> Expression<'a> {
        match expression {
            Expression::UnaryExpression(outer)
                if outer.operator == U::LogicalNot
                    && matches!(&outer.argument, Expression::UnaryExpression(inner) if inner.operator == U::LogicalNot) =>
            {
                let outer = outer.unbox();
                let Expression::UnaryExpression(inner) = outer.argument else {
                    unreachable!()
                };
                inner.unbox().argument
            }
            expression => expression,
        }
    }
    fn negate(&self, expression: Expression<'a>) -> Expression<'a> {
        self.ast.unary(U::LogicalNot, self.truthy(expression))
    }
    fn condition(&self, condition: &Condition) -> Result<Expression<'a>> {
        Ok(match condition {
            Condition::Value(value, nonzero) => self.test(*value, *nonzero)?,
            Condition::Not(condition) => self.negate(self.condition(condition)?),
            Condition::ShortCircuit { all, left, right } => Expression::new_logical_expression(
                SPAN,
                self.condition(left)?,
                if *all { L::And } else { L::Or },
                self.condition(right)?,
                &self.ast.build,
            ),
        })
    }
    fn nodes(&self, nodes: &[Node]) -> Result<Vec<Statement<'a>>> {
        let mut statements = Vec::new();
        for node in nodes {
            match node {
                Node::Block(block) => statements.extend(self.events(*block)?),
                Node::If { condition, yes, no } => statements.push(self.ast.if_(
                    self.truthy(self.condition(condition)?),
                    self.nodes(yes)?,
                    if no.is_empty() {
                        None
                    } else {
                        Some(self.nodes(no)?)
                    },
                )),
                Node::Select {
                    condition,
                    slot,
                    yes,
                    no,
                } => {
                    let value = |choice: &Choice| match choice {
                        Choice::Value(value) => self.value(*value, 0),
                        Choice::Existing => Ok(self.slot(*slot)),
                    };
                    let boolean = |choice: &Choice| match choice {
                        Choice::Value(value) => match self.unit.function.values[value.0].op {
                            Op::Constant(Constant::Bool(value)) => Some(value),
                            _ => None,
                        },
                        Choice::Existing => None,
                    };
                    let condition = self.condition(condition)?;
                    let result = match (boolean(yes), boolean(no)) {
                        (Some(true), Some(false)) => condition,
                        (Some(false), Some(true)) => self.negate(condition),
                        _ => Expression::new_conditional_expression(
                            SPAN,
                            condition,
                            value(yes)?,
                            value(no)?,
                            &self.ast.build,
                        ),
                    };
                    statements.push(self.ast.assign(self.slot(*slot), result));
                }
                Node::Loop { header, body } => {
                    let body = self.ast.block(self.nodes(body)?);
                    let loop_ = Statement::new_while_statement(
                        SPAN,
                        self.ast.boolean(true),
                        body,
                        &self.ast.build,
                    );
                    statements.push(self.ast.label(&format!("__ark_loop_{header}"), loop_));
                }
                Node::Try { id, body, handler } => {
                    let name = format!("__ark_exception_{id}");
                    let mut catch = vec![self.ast.assign(self.ast.id("acc"), self.ast.id(&name))];
                    catch.extend(self.nodes(handler)?);
                    statements.push(self.ast.try_catch(self.nodes(body)?, &name, catch));
                }
                Node::Break(header) => {
                    statements.push(self.ast.jump(&format!("__ark_loop_{header}"), false))
                }
                Node::Continue(header) => {
                    statements.push(self.ast.jump(&format!("__ark_loop_{header}"), true))
                }
                Node::Return(value) => statements.push(
                    self.ast
                        .ret(value.map(|value| self.value(value, 0)).transpose()?),
                ),
                Node::Throw(value) => statements.push(self.ast.throw(self.value(*value, 0)?)),
                Node::Residual {
                    entry,
                    blocks,
                    stop,
                    active,
                    caught_by,
                } => statements.extend(self.residual(*entry, blocks, *stop, *active, *caught_by)?),
            }
        }
        Ok(statements)
    }
    fn residual(
        &self,
        entry: usize,
        blocks: &[usize],
        stop: Option<usize>,
        active: Option<LoopContext>,
        caught_by: Option<usize>,
    ) -> Result<Vec<Statement<'a>>> {
        let name = format!("__ark_pc_{entry}");
        let label = format!("__ark_flow_{entry}");
        let members: HashSet<_> = blocks.iter().copied().collect();
        let transfer = |target: usize| -> Result<Vec<Statement<'a>>> {
            if let Some(active) = active {
                if target == active.header && !members.contains(&target) {
                    return Ok(vec![
                        self.ast
                            .jump(&format!("__ark_loop_{}", active.header), true),
                    ]);
                }
                if Some(target) == active.exit {
                    return Ok(vec![
                        self.ast
                            .jump(&format!("__ark_loop_{}", active.header), false),
                    ]);
                }
            }
            if Some(target) == stop {
                return Ok(vec![self.ast.jump(&label, false)]);
            }
            if !members.contains(&target) {
                return Err(resource("Ark residual edge has no output owner"));
            }
            Ok(vec![
                self.ast
                    .assign(self.ast.id(&name), self.ast.number(target as f64)),
                self.ast.jump(&label, true),
            ])
        };
        let mut cases = Vec::new();
        for id in blocks {
            let block = &self.unit.function.blocks[*id];
            let mut body = self.events(*id)?;
            match &block.end {
                Terminator::Jump(next) => body.extend(transfer(*next)?),
                Terminator::Branch {
                    value,
                    nonzero,
                    yes,
                    no,
                } => body.push(self.ast.if_(
                    self.test(*value, *nonzero)?,
                    transfer(*yes)?,
                    Some(transfer(*no)?),
                )),
                Terminator::Return(value) => body.push(
                    self.ast
                        .ret(value.map(|value| self.value(value, 0)).transpose()?),
                ),
                Terminator::Throw(value) => body.push(self.ast.throw(self.value(*value, 0)?)),
            }
            let handler = match caught_by {
                None => block.handlers.first(),
                Some(outer) => block
                    .handlers
                    .iter()
                    .position(|handler| *handler == outer)
                    .filter(|position| *position != 0)
                    .and_then(|_| block.handlers.first()),
            };
            let can_throw = self.unit.values.may_throw(&self.unit.function, *id);
            if can_throw && let Some(handler) = handler {
                let mut handler_body = vec![
                    self.ast
                        .assign(self.ast.id("acc"), self.ast.id("__ark_exception")),
                ];
                handler_body.extend(transfer(*handler)?);
                body = vec![self.ast.try_catch(body, "__ark_exception", handler_body)];
            }
            cases.push(SwitchCase::new(
                Span::new(block.pc, block.pc + 1),
                Some(self.ast.number(*id as f64)),
                self.ast.vec([self.ast.block(body)]),
                &self.ast.build,
            ));
        }
        let switch = Statement::new_switch_statement(
            SPAN,
            self.ast.id(&name),
            self.ast.vec(cases),
            &self.ast.build,
        );
        let loop_ = Statement::new_while_statement(
            SPAN,
            self.ast.boolean(true),
            self.ast.block(vec![switch]),
            &self.ast.build,
        );
        let mut state = self
            .ast
            .var(&name, Some(self.ast.number(entry as f64)), false);
        self.warnings.borrow_mut().attach(
            &mut state,
            crate::ReconstructionWarning::ControlFlowReconstruction,
        )?;
        Ok(vec![state, self.ast.label(&label, loop_)])
    }
}

fn slot_number(slot: Slot) -> u32 {
    match slot {
        Slot::Acc => 0,
        Slot::Resume => 1,
        Slot::Register(reg) => reg + 2,
    }
}
fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || matches!(ch, b'_' | b'$'))
        && bytes.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'$'))
        && name != "acc"
        && !name.starts_with("__ark_")
        && !(name.len() > 1
            && name.starts_with(['p', 'v', 't'])
            && name[1..].bytes().all(|byte| byte.is_ascii_digit()))
        && !matches!(
            name,
            "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "export"
                | "extends"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "new"
                | "return"
                | "super"
                | "switch"
                | "throw"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
                | "yield"
                | "await"
                | "enum"
                | "let"
                | "static"
                | "implements"
                | "interface"
                | "package"
                | "private"
                | "protected"
                | "public"
                | "null"
                | "true"
                | "false"
                | "arguments"
                | "eval"
        )
}
