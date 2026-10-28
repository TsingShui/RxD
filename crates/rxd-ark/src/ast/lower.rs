use super::nodes::Ast;
use crate::{
    Module,
    analysis::Values,
    ir::lift::unsupported,
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
    collections::{BTreeSet, HashSet},
    io::Write,
};

pub(crate) struct Prepared<'a> {
    pub function: crate::ir::Function<'a>,
    pub values: Values,
    pub body: Body,
}

pub(crate) fn lower(
    units: &[Prepared<'_>],
    module: Option<&Module>,
    budget: Budget,
    main: Option<u32>,
) -> Result<Vec<u8>> {
    let arena = Allocator::new();
    let ast = Ast::new(&arena);
    let mut statements = imports(&ast, units, module)?;
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
        let lower = Lower {
            ast: &ast,
            unit,
            module,
            budget,
            module_mode: main.is_some(),
        };
        statements.push(lower.function()?);
        if arena.used_bytes() > budget.metadata_bytes {
            return Err(resource("Ark AST exceeds metadata admission"));
        }
    }
    if let Some(main) = main {
        statements.extend(super::module::finish(
            &ast,
            module.ok_or_else(|| invalid("Ark record has no module"))?,
            main,
        )?);
    }
    let program = Program::new(
        SPAN,
        SourceType::mjs(),
        "",
        ast.vec([]),
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
    Ok(output.into_bytes())
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
    module: Option<&'f Module>,
    budget: Budget,
    module_mode: bool,
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
        let uses_env = function
            .values
            .iter()
            .any(|value| matches!(value.op, Op::LoadCell { .. } | Op::Closure { .. }))
            || function
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
        let params = (0..function.frame.params)
            .map(|index| format!("p{index}"))
            .collect();
        let expression = if function.frame.arrow {
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
        Ok(self.ast.factory(
            &format!("_ark_capture_{:x}", function.method.offset),
            vec!["__ark_context".into()],
            factory_body,
        ))
    }

    fn slot(&self, slot: Slot) -> Expression<'a> {
        let frame = &self.unit.function.frame;
        match slot {
            Slot::Acc => self.ast.id("acc"),
            Slot::Resume => self.ast.id("__ark_resume"),
            Slot::Register(reg) if Some(reg) == frame.func => self
                .ast
                .id(&format!("__ark_f_{:x}", self.unit.function.method.offset)),
            Slot::Register(reg) if Some(reg) == frame.this_reg => self.this(),
            Slot::Register(reg) if Some(reg) == frame.new_target => self.new_target(),
            Slot::Register(reg) if reg >= frame.first_param => {
                self.ast.id(&format!("p{}", reg - frame.first_param))
            }
            Slot::Register(reg) => self.ast.id(&format!("v{reg}")),
        }
    }
    fn this(&self) -> Expression<'a> {
        if self.unit.function.frame.arrow {
            self.ast.property(self.ast.id("__ark_context"), "receiver")
        } else {
            Expression::new_this_expression(SPAN, &self.ast.build)
        }
    }
    fn new_target(&self) -> Expression<'a> {
        if self.unit.function.frame.arrow {
            self.ast.property(self.ast.id("__ark_context"), "newTarget")
        } else {
            Expression::new_new_target(SPAN, &self.ast.build)
        }
    }
    fn context(&self) -> Expression<'a> {
        self.ast.object(vec![
            (self.ast.string("env"), self.ast.id("__ark_env")),
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
            Op::Function => self
                .ast
                .id(&format!("__ark_f_{:x}", self.unit.function.method.offset)),
            Op::Global(name) => {
                let name = name
                    .scalar_text()
                    .ok_or_else(|| unsupported(value.pc, "non-scalar global binding"))?;
                if !identifier(name) {
                    return Err(unsupported(
                        value.pc,
                        "global binding conflicts with generated names",
                    ));
                }
                self.ast.id(name)
            }
            Op::GlobalObject => self.ast.id("globalThis"),
            Op::Unary(op, input) => self.ast.unary(*op, self.value(*input, depth + 1)?),
            Op::Binary(op, left, right) => self.ast.binary(
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
            Op::Import(index) => self.ast.id(&format!("__ark_i_{index}")),
            Op::Namespace(index) => self.ast.id(&format!("__ark_ns_{index}")),
            Op::Module(index) => self.module(*index)?,
            Op::Closure { method, length } => {
                let call = self.ast.call(
                    self.ast.id(&format!("_ark_capture_{method:x}")),
                    vec![self.context()],
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
            Op::Iterator(input) => {
                let object = self.value(*input, depth + 1)?;
                let key = self.ast.property(self.ast.id("Symbol"), "iterator");
                self.ast.call(self.ast.member(object, key), Vec::new())
            }
            Op::Keys(_) | Op::Next(_) => {
                return Err(unsupported(
                    value.pc,
                    "for-in iterator requires loop normalization",
                ));
            }
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
                if self.unit.function.frame.arrow {
                    return Err(unsupported(value.pc, "arrow rest argument frame"));
                }
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
                    if self.unit.values.materialize[id.0] {
                        statements.push(self.ast.var(
                            &format!("t{}", id.0),
                            Some(self.definition(*id, 0)?),
                            true,
                        ));
                    } else if value.effects.ordered() && self.unit.values.uses[id.0] == 0 {
                        statements.push(self.ast.expr(self.definition(*id, 0)?));
                    }
                }
                Event::Action { pc, action } if self.unit.values.keep[block][index] => {
                    let mut action = self.action(action)?;
                    *action.span_mut() = Span::new(*pc, *pc + 1);
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
            Action::Set {
                object,
                key,
                value,
                own,
            } => {
                if *own != StoreKind::Assign {
                    if *own == StoreKind::ClassSensitive && !self.unit.values.ordinary[object.0] {
                        return Err(unsupported(
                            self.unit.function.values[object.0].pc,
                            "class-sensitive receiver is not proven ordinary",
                        ));
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
                    vec![
                        self.ast.throw(
                            self.ast.construct(
                                self.ast.id("ReferenceError"),
                                vec![self.ast.name(name)],
                            ),
                        ),
                    ],
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
                        vec![self.ast.string("value is not an object")],
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
        Ok(vec![
            self.ast
                .var(&name, Some(self.ast.number(entry as f64)), false),
            self.ast.label(&label, loop_),
        ])
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
