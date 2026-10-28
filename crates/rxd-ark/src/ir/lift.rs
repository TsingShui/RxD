use super::*;
use crate::{
    Abc, EntityKind, Instruction, LiteralValue, Operand,
    reader::{invalid, resource},
};
use rxd_core::{Error, Result};
use std::collections::HashMap;

#[path = "extra.rs"]
mod extra;
#[path = "legacy.rs"]
mod legacy;

pub(crate) fn unsupported(pc: u32, message: impl std::fmt::Display) -> Error {
    Error::new(
        "DECOMPILE_FAILED",
        format!("Ark IR at PC {pc:#x}: {message}"),
    )
}

struct Builder<'a, 's> {
    abc: &'s Abc<'a>,
    method: &'s crate::Method,
    values: Vec<Value>,
    slots: HashMap<Slot, ValueId>,
    events: Vec<Event>,
    block: usize,
    pc: u32,
    generator: bool,
    raw_protocol: bool,
    literal_stack: Vec<u32>,
    definition_env: Option<ValueId>,
}

impl Builder<'_, '_> {
    fn value(&mut self, op: Op, effects: Effects) -> ValueId {
        let id = ValueId(self.values.len());
        self.values.push(Value {
            pc: self.pc,
            block: self.block,
            op,
            effects,
        });
        self.events.push(Event::Define(id));
        id
    }
    fn read(&mut self, slot: Slot) -> ValueId {
        if let Some(id) = self.slots.get(&slot) {
            return *id;
        }
        let id = self.value(Op::Read(slot), Effects::PURE);
        self.slots.insert(slot, id);
        id
    }
    fn reg(&mut self, instruction: &Instruction, index: usize) -> Result<ValueId> {
        Ok(self.read(Slot::Register(instruction.register(index)?)))
    }
    fn acc(&mut self) -> ValueId {
        self.read(Slot::Acc)
    }
    fn store(&mut self, slot: Slot, value: ValueId) {
        self.slots.insert(slot, value);
        self.action(Action::Write(slot, value));
    }
    fn action(&mut self, action: Action) {
        self.events.push(Event::Action {
            pc: self.pc,
            action,
        });
    }
    fn result(&mut self, op: Op, effects: Effects) {
        let id = self.value(op, effects);
        self.store(Slot::Acc, id);
    }
    fn constant(&mut self, value: Constant) {
        self.result(Op::Constant(value), Effects::PURE);
    }
    fn key(&self, instruction: &Instruction, index: usize) -> Result<Key> {
        Ok(Key::Name(
            self.abc
                .name(instruction.entity(index, EntityKind::String)?)?,
        ))
    }
    fn range(&mut self, start: u32, count: u32) -> Result<Vec<ValueId>> {
        self.abc.check_storage::<ValueId>(count as usize)?;
        let end = start
            .checked_add(count)
            .ok_or_else(|| invalid("Ark argument range overflow"))?;
        Ok((start..end)
            .map(|reg| self.read(Slot::Register(reg)))
            .collect())
    }
    fn buffer(&mut self, instruction: &Instruction, object: bool) -> Result<Op> {
        let literals = self
            .abc
            .literals(instruction.entity(1, EntityKind::Literal)?)?;
        let mut entries = Vec::new();
        let mut accessors = Vec::new();
        let mut values = Vec::new();
        let mut index = 0;
        while index < literals.len() {
            let key = if object {
                let key = match &literals[index].value {
                    LiteralValue::String(name) => Key::Name(Arc::clone(name)),
                    LiteralValue::Integer(index) => Key::Index(*index),
                    _ => Key::Value(self.literal(&literals[index])?),
                };
                index += 1;
                Some(key)
            } else {
                None
            };
            let literal = literals
                .get(index)
                .ok_or_else(|| invalid("object key without value"))?;
            index += 1;
            let value = if let LiteralValue::Method(method) = literal.value {
                let length = if let Some(crate::Literal {
                    value: LiteralValue::Affiliate(length),
                    ..
                }) = literals.get(index)
                {
                    index += 1;
                    Some(u32::from(*length))
                } else {
                    None
                };
                self.value(
                    Op::Closure {
                        method,
                        length,
                        environment: self.definition_env,
                    },
                    Effects::READ,
                )
            } else {
                self.literal(literal)?
            };
            if let Some(key) = key {
                accessors.push(literal.tag == 8);
                entries.push((key, value));
            } else {
                values.push(value);
            }
        }
        Ok(if object && accessors.iter().any(|value| *value) {
            let mut properties = Vec::new();
            for ((key, value), accessor) in entries.into_iter().zip(accessors) {
                let key = match key {
                    Key::Name(name) => {
                        self.value(Op::Constant(Constant::String(name)), Effects::PURE)
                    }
                    Key::Index(index) => self.number(index),
                    Key::Value(value) => value,
                };
                let flag = self.value(Op::Constant(Constant::Bool(accessor)), Effects::PURE);
                properties.push(self.value(Op::Array(vec![key, value, flag]), Effects::READ));
            }
            let properties = self.value(Op::Array(properties), Effects::READ);
            Op::Intrinsic(Intrinsic::ObjectLiteral, vec![properties])
        } else if object {
            Op::Object(entries)
        } else {
            Op::Array(values)
        })
    }
    fn literal(&mut self, literal: &crate::Literal) -> Result<ValueId> {
        let op = match &literal.value {
            LiteralValue::Byte(value) if literal.tag == 1 => {
                Op::Constant(Constant::Bool(*value != 0))
            }
            LiteralValue::Byte(_) if literal.tag == 0xff => Op::Constant(Constant::Null),
            LiteralValue::Integer(value) if literal.tag == 0x17 => {
                let offset = self.abc.literal_index.get(*value)?;
                return self.literal_array(offset);
            }
            LiteralValue::Integer(value) => {
                Op::Constant(Constant::Number(f64::from(*value as i32)))
            }
            LiteralValue::Double(bits) => Op::Constant(Constant::Number(f64::from_bits(*bits))),
            LiteralValue::Float(bits) => {
                Op::Constant(Constant::Number(f64::from(f32::from_bits(*bits))))
            }
            LiteralValue::String(name) => Op::Constant(Constant::String(Arc::clone(name))),
            LiteralValue::Array(offset) => return self.literal_array(*offset),
            LiteralValue::Method(method) => Op::Closure {
                method: *method,
                length: None,
                environment: self.definition_env,
            },
            LiteralValue::Affiliate(length) => Op::Constant(Constant::Number(f64::from(*length))),
            LiteralValue::Byte(_) if literal.tag == 8 => Op::Intrinsic(Intrinsic::Accessor, vec![]),
            LiteralValue::Byte(value) => Op::Constant(Constant::Number(f64::from(*value))),
        };
        let effects = if matches!(op, Op::Closure { .. } | Op::Intrinsic(..)) {
            Effects::CALL
        } else {
            Effects::PURE
        };
        Ok(self.value(op, effects))
    }
    fn literal_array(&mut self, offset: u32) -> Result<ValueId> {
        if self.literal_stack.contains(&offset) {
            return Err(invalid("cyclic Ark literal arrays"));
        }
        if self.literal_stack.len() >= self.abc.budget.ast_depth / 4 {
            return Err(resource("Ark literal nesting exceeds depth guard"));
        }
        self.literal_stack.push(offset);
        let literals = self.abc.literals(offset)?;
        let mut values = Vec::new();
        let mut index = 0;
        while index < literals.len() {
            let value = if let LiteralValue::Method(method) = literals[index].value {
                let length = if let Some(crate::Literal {
                    value: LiteralValue::Affiliate(length),
                    ..
                }) = literals.get(index + 1)
                {
                    index += 1;
                    Some(u32::from(*length))
                } else {
                    None
                };
                self.value(
                    Op::Closure {
                        method,
                        length,
                        environment: self.definition_env,
                    },
                    Effects::CALL,
                )
            } else {
                self.literal(&literals[index])?
            };
            values
                .try_reserve(1)
                .map_err(|_| resource("Ark expanded literal allocation failed"))?;
            values.push(value);
            index += 1;
        }
        self.literal_stack.pop();
        Ok(self.value(Op::Array(values), Effects::READ))
    }
    fn class_members(
        &mut self,
        offset: u32,
        sendable: bool,
    ) -> Result<Vec<super::classes::ClassMember>> {
        super::classes::members(self.abc, offset, sendable)?
            .into_iter()
            .map(|member| {
                let name = match &member.key.value {
                    LiteralValue::String(name) => Key::Name(Arc::clone(name)),
                    LiteralValue::Integer(index) => Key::Index(*index),
                    _ => Key::Value(self.literal(&member.key)?),
                };
                let (method, value) = if let LiteralValue::Method(method) = member.value.value {
                    (Some(method), None)
                } else {
                    (None, Some(self.literal(&member.value)?))
                };
                Ok(super::classes::ClassMember {
                    name,
                    method,
                    value,
                    length: member.length,
                    is_static: member.is_static,
                    accessor: member.value.tag == 8,
                })
            })
            .collect()
    }
    fn instruction(&mut self, instruction: &Instruction) -> Result<()> {
        use BinaryOperator as B;
        use UnaryOperator as U;
        self.pc = instruction.pc;
        let name = instruction.opcode.name;
        if self.legacy(instruction)? || self.extended(instruction)? {
            return Ok(());
        }
        match name {
            "nop" => {}
            "lda" => {
                let value = self.reg(instruction, 0)?;
                self.store(Slot::Acc, value);
            }
            "sta" => {
                let value = self.acc();
                self.store(Slot::Register(instruction.register(0)?), value);
            }
            "mov" => {
                let value = self.reg(instruction, 1)?;
                self.store(Slot::Register(instruction.register(0)?), value);
            }
            "ldai" => {
                let Some(Operand::Signed(value)) = instruction.operands.first() else {
                    return Err(invalid("invalid ldai"));
                };
                self.constant(Constant::Number(*value as f64));
            }
            "fldai" => {
                let Some(Operand::Float(bits)) = instruction.operands.first() else {
                    return Err(invalid("invalid fldai"));
                };
                self.constant(Constant::Number(f64::from_bits(*bits)));
            }
            "lda.str" => self.constant(Constant::String(
                self.abc.name(instruction.entity(0, EntityKind::String)?)?,
            )),
            "ldtrue" => self.constant(Constant::Bool(true)),
            "ldfalse" => self.constant(Constant::Bool(false)),
            "ldnull" => self.constant(Constant::Null),
            "ldundefined" => self.constant(Constant::Undefined),
            "ldhole" => self.constant(Constant::Hole),
            "ldnan" => self.constant(Constant::Number(f64::NAN)),
            "ldinfinity" => self.constant(Constant::Number(f64::INFINITY)),
            "ldthis" => self.result(Op::This, Effects::PURE),
            "ldnewtarget" => self.result(Op::NewTarget, Effects::PURE),
            "ldfunction" => self.result(Op::Function, Effects::PURE),
            "ldglobal" => self.result(Op::GlobalObject, Effects::READ),
            "tryldglobalbyname" => self.result(
                Op::Global(self.abc.name(instruction.entity(1, EntityKind::String)?)?),
                Effects::READ,
            ),
            "ldobjbyname" => {
                let object = self.acc();
                let key = self.key(instruction, 1)?;
                self.result(Op::Get(object, key), Effects::READ);
            }
            "ldobjbyvalue" => {
                let object = self.reg(instruction, 1)?;
                let key = self.acc();
                self.result(Op::Get(object, Key::Value(key)), Effects::READ);
            }
            "ldobjbyindex" | "wide.ldobjbyindex" => {
                let object = self.acc();
                let key = Key::Index(instruction.unsigned(usize::from(name == "ldobjbyindex"))?);
                self.result(Op::Get(object, key), Effects::READ);
            }
            "stobjbyname" | "definefieldbyname" | "definepropertybyname" | "stownbyname" => {
                let object = self.reg(instruction, 2)?;
                let key = self.key(instruction, 1)?;
                let value = self.acc();
                let own = match name {
                    "stobjbyname" => StoreKind::Assign,
                    "stownbyname" => StoreKind::ClassSensitive,
                    _ => StoreKind::Define,
                };
                self.action(Action::Set {
                    object,
                    key,
                    value,
                    own,
                });
            }
            "stobjbyvalue" | "stownbyvalue" | "callruntime.definefieldbyvalue" => {
                let field = name.starts_with("callruntime.");
                let object = self.reg(instruction, if field { 2 } else { 1 })?;
                let key = self.reg(instruction, if field { 1 } else { 2 })?;
                let value = self.acc();
                let own = match name {
                    "stobjbyvalue" => StoreKind::Assign,
                    "stownbyvalue" => StoreKind::ClassSensitive,
                    _ => StoreKind::Define,
                };
                self.action(Action::Set {
                    object,
                    key: Key::Value(key),
                    value,
                    own,
                });
            }
            "stobjbyindex"
            | "wide.stobjbyindex"
            | "stownbyindex"
            | "wide.stownbyindex"
            | "callruntime.definefieldbyindex" => {
                let wide = name.starts_with("wide.");
                let field = name.starts_with("callruntime.");
                let object = self.reg(instruction, if field { 2 } else { usize::from(!wide) })?;
                let key = Key::Index(instruction.unsigned(if wide || field { 1 } else { 2 })?);
                let value = self.acc();
                let own = if field {
                    StoreKind::Define
                } else if name.contains("stown") {
                    StoreKind::ClassSensitive
                } else {
                    StoreKind::Assign
                };
                self.action(Action::Set {
                    object,
                    key,
                    value,
                    own,
                });
            }
            "createemptyobject" => self.result(Op::Object(Vec::new()), Effects::READ),
            "createemptyarray" => self.result(Op::Array(Vec::new()), Effects::READ),
            "createobjectwithbuffer" | "createarraywithbuffer" => {
                let op = self.buffer(instruction, name == "createobjectwithbuffer")?;
                self.result(op, Effects::READ);
            }
            "newlexenv" | "wide.newlexenv" | "newlexenvwithname" | "wide.newlexenvwithname" => {
                self.action(Action::PushEnv(instruction.unsigned(0)?));
                self.result(Op::Environment, Effects::READ);
            }
            "poplexenv" => self.action(Action::PopEnv),
            "asyncfunctionawaituncaught" if self.raw_protocol => {
                let state = self.reg(instruction, 0)?;
                let value = self.acc();
                self.intrinsic(Intrinsic::AsyncAwait, vec![state, value], true);
            }
            "asyncfunctionenter" => self.intrinsic(Intrinsic::AsyncEnter, vec![], true),
            "asyncfunctionresolve" | "asyncfunctionreject" | "asyncgeneratorreject" => {
                let state = self.reg(instruction, 0)?;
                let value = self.acc();
                let kind = match name {
                    "asyncfunctionresolve" => Intrinsic::AsyncResolve,
                    "asyncfunctionreject" => Intrinsic::AsyncReject,
                    _ => Intrinsic::AsyncGenReject,
                };
                self.intrinsic(kind, vec![state, value], true);
            }
            "asyncgeneratorresolve" => {
                let state = self.reg(instruction, 0)?;
                let value = self.reg(instruction, 1)?;
                let done = self.reg(instruction, 2)?;
                self.value(Op::AsyncYield(state, value, done), Effects::SUSPEND);
                self.constant(Constant::Undefined);
            }
            "setgeneratorstate" => {
                let state = self.acc();
                let value = self.number(instruction.unsigned(0)?);
                self.intrinsic(Intrinsic::GeneratorState, vec![state, value], false);
            }
            "suspendgenerator" if !self.generator => {
                let state = self.reg(instruction, 0)?;
                let value = self.acc();
                self.intrinsic(Intrinsic::AsyncPause, vec![state, value], false);
            }
            "asyncfunctionawaituncaught" => {
                let value = self.acc();
                let result = self.value(Op::Await(value), Effects::SUSPEND);
                self.store(Slot::Resume, result);
            }
            "creategeneratorobj" | "createasyncgeneratorobj" => {
                self.result(Op::Generator, Effects::READ)
            }
            "suspendgenerator" if self.generator => {
                let generator = self.reg(instruction, 0)?;
                let value = self.acc();
                self.value(Op::Suspend(generator, value), Effects::SUSPEND);
            }
            "createiterresultobj" => {
                let value = self.reg(instruction, 0)?;
                let done = self.reg(instruction, 1)?;
                self.result(Op::IterResult(value, done), Effects::READ);
            }
            "resumegenerator" | "getresumemode" if self.raw_protocol => {
                let generator = self.acc();
                self.result(
                    Op::GeneratorResume(generator, name == "getresumemode"),
                    Effects::READ,
                );
            }
            "resumegenerator" => {
                let value = self.read(Slot::Resume);
                self.store(Slot::Acc, value);
            }
            "getresumemode" => self.constant(Constant::Number(0.0)),
            "apply" => {
                let function = self.acc();
                let receiver = self.reg(instruction, 1)?;
                let args = self.reg(instruction, 2)?;
                self.result(Op::Apply(function, receiver, args), Effects::CALL);
            }
            "ldlexvar" | "wide.ldlexvar" => self.result(
                Op::LoadCell {
                    depth: instruction.unsigned(0)?,
                    index: instruction.unsigned(1)?,
                },
                Effects::READ,
            ),
            "stlexvar" | "wide.stlexvar" => {
                let value = self.acc();
                self.action(Action::StoreCell {
                    depth: instruction.unsigned(0)?,
                    index: instruction.unsigned(1)?,
                    value,
                });
            }
            "throw.undefinedifholewithname" => {
                let value = self.acc();
                self.action(Action::CheckHole(
                    value,
                    self.abc.name(instruction.entity(0, EntityKind::String)?)?,
                ));
            }
            "throw.ifnotobject" => {
                let value = self.reg(instruction, 0)?;
                self.action(Action::CheckObject(value));
            }
            "ldexternalmodulevar" | "wide.ldexternalmodulevar" => {
                self.result(Op::Import(instruction.unsigned(0)?), Effects::READ)
            }
            "ldlocalmodulevar" | "wide.ldlocalmodulevar" => {
                self.result(Op::Module(instruction.unsigned(0)?), Effects::READ)
            }
            "getmodulenamespace" | "wide.getmodulenamespace" => {
                self.result(Op::Namespace(instruction.unsigned(0)?), Effects::READ)
            }
            "stmodulevar" | "wide.stmodulevar" => {
                let value = self.acc();
                self.action(Action::StoreModule(instruction.unsigned(0)?, value));
            }
            "definefunc" => {
                let method = instruction.entity(1, EntityKind::Method)?;
                self.result(
                    Op::Closure {
                        method,
                        length: Some(instruction.unsigned(2)?),
                        environment: None,
                    },
                    Effects::CALL,
                );
            }
            "defineclasswithbuffer" => {
                let constructor = instruction.entity(1, EntityKind::Method)?;
                let members =
                    self.class_members(instruction.entity(2, EntityKind::Literal)?, false)?;
                let base = self.reg(instruction, 4)?;
                self.result(
                    Op::Class {
                        constructor,
                        name: Arc::clone(&self.abc.method(constructor)?.name),
                        base,
                        environment: self.definition_env,
                        length: instruction.unsigned(3)?,
                        members,
                    },
                    Effects::CALL,
                );
            }
            "definemethod" => {
                let home = self.acc();
                let method = instruction.entity(1, EntityKind::Method)?;
                self.result(
                    Op::Method {
                        method,
                        name: Arc::clone(&self.abc.method(method)?.name),
                        length: instruction.unsigned(2)?,
                        home,
                    },
                    Effects::CALL,
                );
            }
            "definegettersetterbyvalue" => {
                let object = self.reg(instruction, 0)?;
                let key = self.reg(instruction, 1)?;
                let getter = self.reg(instruction, 2)?;
                let setter = self.reg(instruction, 3)?;
                let set_name = self.acc();
                self.action(Action::Accessors {
                    object,
                    key,
                    getter,
                    setter,
                    set_name,
                });
                self.store(Slot::Acc, object);
            }
            "throw.ifsupernotcorrectcall" => {
                let index = instruction.unsigned(0)?;
                if index > 1 {
                    return Err(invalid("invalid Ark super check"));
                }
                let value = self.acc();
                self.action(Action::CheckSuper(value, index == 1));
            }
            "supercallthisrange"
            | "wide.supercallthisrange"
            | "supercallarrowrange"
            | "wide.supercallarrowrange" => {
                let wide = name.starts_with("wide.");
                let count = instruction.unsigned(usize::from(!wide))?;
                let start = instruction.register(if wide { 1 } else { 2 })?;
                let args = self.range(start, count)?;
                let function = name.ends_with("supercallarrowrange").then(|| self.acc());
                self.result(
                    Op::SuperCall {
                        function,
                        args,
                        spread: None,
                    },
                    Effects::CALL,
                );
            }
            "supercallspread" => {
                let function = Some(self.acc());
                let spread = Some(self.reg(instruction, 1)?);
                self.result(
                    Op::SuperCall {
                        function,
                        args: vec![],
                        spread,
                    },
                    Effects::CALL,
                );
            }
            "ldsuperbyname" | "ldsuperbyvalue" => {
                let (receiver, key) = if name == "ldsuperbyname" {
                    (self.acc(), self.key(instruction, 1)?)
                } else {
                    (self.reg(instruction, 1)?, Key::Value(self.acc()))
                };
                self.result(Op::SuperGet(receiver, key), Effects::CALL);
            }
            "stsuperbyname" | "stsuperbyvalue" => {
                let (receiver, key) = if name == "stsuperbyname" {
                    (self.reg(instruction, 2)?, self.key(instruction, 1)?)
                } else {
                    (
                        self.reg(instruction, 1)?,
                        Key::Value(self.reg(instruction, 2)?),
                    )
                };
                let value = self.acc();
                self.action(Action::SuperSet {
                    receiver,
                    key,
                    value,
                });
            }
            "callarg0" | "callarg1" | "callargs2" | "callargs3" | "callthis0" | "callthis1"
            | "callthis2" | "callthis3" => {
                let callee = self.acc();
                let receiver = name
                    .starts_with("callthis")
                    .then(|| self.reg(instruction, 1))
                    .transpose()?;
                let start = if receiver.is_some() { 2 } else { 1 };
                let mut args = Vec::new();
                for index in start..instruction.operands.len() {
                    args.push(self.reg(instruction, index)?);
                }
                self.result(
                    Op::Call {
                        callee,
                        receiver,
                        args,
                    },
                    Effects::CALL,
                );
            }
            "callrange" | "wide.callrange" | "callthisrange" | "wide.callthisrange"
            | "newobjrange" | "wide.newobjrange" => {
                let wide = name.starts_with("wide.");
                let start = instruction.register(if wide { 1 } else { 2 })?;
                let count = instruction.unsigned(usize::from(!wide))?;
                if name.ends_with("newobjrange") {
                    let callee = self.read(Slot::Register(start));
                    let args = self.range(start + 1, count - 1)?;
                    self.result(Op::Construct { callee, args }, Effects::CALL);
                } else {
                    let callee = self.acc();
                    let receiver = name
                        .ends_with("callthisrange")
                        .then(|| self.read(Slot::Register(start)));
                    let args = self.range(start + u32::from(receiver.is_some()), count)?;
                    self.result(
                        Op::Call {
                            callee,
                            receiver,
                            args,
                        },
                        Effects::CALL,
                    );
                }
            }
            "typeof"
            | "neg"
            | "not"
            | "tonumber"
            | "istrue"
            | "isfalse"
            | "callruntime.istrue"
            | "callruntime.isfalse" => {
                let value = self.acc();
                let operator = match name {
                    "typeof" => U::Typeof,
                    "neg" => U::UnaryNegation,
                    "not" => U::BitwiseNot,
                    "tonumber" => U::UnaryPlus,
                    _ => U::LogicalNot,
                };
                let effects = if matches!(operator, U::LogicalNot | U::Typeof) {
                    Effects::PURE
                } else {
                    Effects::READ
                };
                if name.ends_with("istrue") {
                    let inner = self.value(Op::Unary(U::LogicalNot, value), Effects::PURE);
                    self.result(Op::Unary(U::LogicalNot, inner), Effects::PURE);
                } else {
                    self.result(Op::Unary(operator, value), effects);
                }
            }
            "tonumeric" => {
                let value = self.acc();
                self.result(Op::ToNumeric(value), Effects::READ);
            }
            "inc" | "dec" => {
                let value = self.acc();
                self.result(Op::Update(value, name == "inc"), Effects::READ);
            }
            "add2" | "sub2" | "mul2" | "div2" | "mod2" | "exp" | "eq" | "noteq" | "less"
            | "lesseq" | "greater" | "greatereq" | "stricteq" | "strictnoteq" | "shl2" | "shr2"
            | "ashr2" | "and2" | "or2" | "xor2" | "instanceof" | "isin" => {
                let operator = match name {
                    "add2" => B::Addition,
                    "sub2" => B::Subtraction,
                    "mul2" => B::Multiplication,
                    "div2" => B::Division,
                    "mod2" => B::Remainder,
                    "exp" => B::Exponential,
                    "eq" => B::Equality,
                    "noteq" => B::Inequality,
                    "less" => B::LessThan,
                    "lesseq" => B::LessEqualThan,
                    "greater" => B::GreaterThan,
                    "greatereq" => B::GreaterEqualThan,
                    "stricteq" => B::StrictEquality,
                    "strictnoteq" => B::StrictInequality,
                    "shl2" => B::ShiftLeft,
                    "shr2" => B::ShiftRightZeroFill,
                    "ashr2" => B::ShiftRight,
                    "and2" => B::BitwiseAnd,
                    "or2" => B::BitwiseOR,
                    "xor2" => B::BitwiseXOR,
                    "instanceof" => B::Instanceof,
                    "isin" => B::In,
                    _ => unreachable!(),
                };
                let left = self.reg(instruction, 1)?;
                let right = self.acc();
                let effects = if matches!(operator, B::StrictEquality | B::StrictInequality) {
                    Effects::PURE
                } else {
                    Effects::READ
                };
                self.result(Op::Binary(operator, left, right), effects);
            }
            "delobjprop" => {
                let object = self.reg(instruction, 0)?;
                let key = self.acc();
                self.result(Op::Delete(object, key), Effects::WRITE);
            }
            "copydataproperties" => {
                let target = self.reg(instruction, 0)?;
                let source = self.acc();
                self.result(Op::CopyData(target, source), Effects::WRITE);
            }
            "starrayspread" => {
                let dst = self.reg(instruction, 0)?;
                let index = self.reg(instruction, 1)?;
                let src = self.acc();
                self.result(Op::ArraySpread(dst, index, src), Effects::CALL);
            }
            "copyrestargs" | "wide.copyrestargs" => {
                self.result(Op::RestArgs(instruction.unsigned(0)?), Effects::READ)
            }
            "getiterator" | "getpropiterator" | "getnextpropname" | "dynamicimport" => {
                let value = if name == "getnextpropname" {
                    self.reg(instruction, 0)?
                } else {
                    self.acc()
                };
                let op = match name {
                    "getiterator" => Op::Iterator(value),
                    "getpropiterator" => Op::Keys(value),
                    "getnextpropname" => Op::Next(value),
                    _ => Op::DynamicImport(value),
                };
                self.result(op, Effects::CALL);
            }
            _ => return Err(unsupported(self.pc, name)),
        }
        Ok(())
    }
}

pub(crate) fn lift<'a>(abc: &Abc<'a>, method: &Method) -> Result<Function<'a>> {
    let mut decoded = abc.decode(method)?;
    let call_type = abc.method_call_type(method)?;
    let kind = ((method.flags >> 8) & 255) as u8;
    let frame_warning = !matches!(kind, 0 | 1 | 2 | 4 | 6 | 7);
    let async_generator = decoded.instructions.iter().any(|instruction| {
        matches!(
            instruction.opcode.name,
            "createasyncgeneratorobj"
                | "asyncgeneratorresolve"
                | "asyncgeneratorreject"
                | "deprecated.asyncgeneratorreject"
        )
    });
    let asynchronous = matches!(kind, 4 | 6)
        || decoded
            .instructions
            .iter()
            .any(|instruction| instruction.opcode.name.contains("asyncfunction"));
    let generators = decoded
        .instructions
        .iter()
        .filter(|instruction| {
            matches!(
                instruction.opcode.name,
                "creategeneratorobj" | "createasyncgeneratorobj"
            )
        })
        .count();
    let generator = generators != 0 || async_generator;
    let protocol = if asynchronous && !generator {
        match super::async_frame::normalize(&mut decoded) {
            Ok(protocol) => Some(protocol),
            Err(error) if error.code == "DECOMPILE_FAILED" => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    let raw_async = asynchronous && protocol.is_none();
    let raw_protocol = protocol.is_none();
    let typed_catches = decoded
        .code
        .tries
        .iter()
        .any(|region| region.catches.iter().any(|catch| catch.type_index != 0));
    let mut ranges: Vec<_> = decoded
        .code
        .tries
        .iter()
        .map(|region| (region.pc, region.pc + region.length))
        .collect();
    ranges.sort_unstable();
    let residual_only = typed_catches
        || ranges
            .windows(2)
            .any(|pair| pair[1].0 < pair[0].1 && pair[0].1 < pair[1].1);
    let graph = Graph::build(&decoded, abc.budget)?;
    let mut next = decoded.code.vregs;
    let func = (call_type & 8 != 0).then(|| {
        let register = next;
        next += 1;
        register
    });
    let new_target = (call_type & 2 != 0).then(|| {
        let register = next;
        next += 1;
        register
    });
    let this_reg = (call_type & 1 != 0).then(|| {
        let register = next;
        next += 1;
        register
    });
    let params = decoded
        .code
        .args
        .checked_sub(next - decoded.code.vregs)
        .ok_or_else(|| invalid("Ark call frame exceeds arguments"))?;
    let frame = Frame {
        locals: decoded.code.vregs,
        params,
        func,
        new_target,
        this_reg,
        first_param: next,
        arrow: matches!(kind, 2 | 6),
        asynchronous,
        constructor: !matches!(kind, 2 | 6)
            && decoded.instructions.iter().any(|instruction| {
                matches!(
                    instruction.opcode.name,
                    "supercallthisrange" | "wide.supercallthisrange" | "supercallspread"
                )
            }),
        generator,
        async_generator: async_generator || generator && asynchronous,
        multi_generator: generators > 1,
        raw_async,
        frame_warning,
        typed_catches,
        residual_only,
    };
    let mut builder = Builder {
        abc,
        method,
        values: Vec::new(),
        slots: HashMap::new(),
        events: Vec::new(),
        block: 0,
        pc: 0,
        generator,
        raw_protocol,
        literal_stack: Vec::new(),
        definition_env: None,
    };
    let mut blocks = Vec::new();
    for (index, block) in graph.blocks.iter().enumerate() {
        builder.block = index;
        builder.slots.clear();
        builder.events.clear();
        let slice = &decoded.instructions[block.first..block.end];
        let last = slice.last().ok_or_else(|| invalid("empty Ark block"))?;
        let terminal = last.opcode.is_jump() || crate::instructions::is_terminator(last);
        for instruction in &slice[..slice.len() - usize::from(terminal)] {
            if protocol
                .as_ref()
                .is_some_and(|frame| frame.skip.contains(&instruction.pc))
            {
                continue;
            }
            builder.instruction(instruction)?;
        }
        builder.pc = last.pc;
        let end = if last.opcode.is_jump() {
            let yes = graph.target(
                &decoded.instructions,
                last.branch
                    .ok_or_else(|| invalid("Ark jump lacks target"))?,
            )?;
            if last.opcode.is_conditional() {
                let (value, nonzero) = if matches!(last.opcode.name, "jeqz" | "jnez") {
                    (builder.acc(), last.opcode.name == "jnez")
                } else {
                    (builder.branch_compare(last)?, true)
                };
                Terminator::Branch {
                    value,
                    nonzero,
                    yes,
                    no: index + 1,
                }
            } else {
                Terminator::Jump(yes)
            }
        } else if last.opcode.name == "return" {
            Terminator::Return(Some(builder.acc()))
        } else if last.opcode.name == "returnundefined" {
            Terminator::Return(None)
        } else if last.opcode.name == "throw" {
            Terminator::Throw(builder.acc())
        } else if last.opcode.name == "throw.notexists" {
            let error = builder.value(Op::TypeError("Throw method is not defined"), Effects::CALL);
            Terminator::Throw(error)
        } else if matches!(
            last.opcode.name,
            "throw.patternnoncoercible" | "throw.deletesuperproperty" | "throw.constassignment"
        ) {
            let (kind, message, value) = match last.opcode.name {
                "throw.patternnoncoercible" => ("TypeError", "objectnotcoercible", None),
                "throw.deletesuperproperty" => {
                    ("ReferenceError", "Can not delete super property", None)
                }
                _ => (
                    "TypeError",
                    "Assignment to const variable ",
                    Some(builder.reg(last, 0)?),
                ),
            };
            let error = builder.value(Op::NamedError(kind, message, value), Effects::CALL);
            Terminator::Throw(error)
        } else if terminal {
            return Err(invalid(
                "Ark instruction has inconsistent terminal metadata",
            ));
        } else {
            Terminator::Jump(index + 1)
        };
        let pc = slice[0].pc;
        let mut handlers = Vec::new();
        // The pinned dynamic runtime selects the first matching entry in encoded table order.
        for region in &decoded.code.tries {
            if region.pc <= pc && pc < region.pc + region.length {
                for handler in &region.catches {
                    handlers.push(graph.target(&decoded.instructions, handler.pc)?);
                }
            }
        }
        abc.check_storage::<Value>(builder.values.len())?;
        blocks
            .try_reserve(1)
            .map_err(|_| resource("Ark IR block allocation failed"))?;
        blocks.push(Block {
            pc,
            events: std::mem::take(&mut builder.events),
            end,
            handlers,
        });
    }
    Ok(Function {
        method: method.clone(),
        code: decoded.code,
        frame,
        graph,
        blocks,
        values: builder.values,
    })
}
