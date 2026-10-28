use super::*;
use crate::{
    Abc, EntityKind, Instruction, LiteralValue, Operand,
    reader::{invalid, resource},
};
use rxd_core::{Error, Result};
use std::collections::HashMap;

pub(crate) fn unsupported(pc: u32, message: impl std::fmt::Display) -> Error {
    Error::new(
        "DECOMPILE_FAILED",
        format!("Ark IR at PC {pc:#x}: {message}"),
    )
}

struct Builder<'a, 's> {
    abc: &'s Abc<'a>,
    values: Vec<Value>,
    slots: HashMap<Slot, ValueId>,
    events: Vec<Event>,
    block: usize,
    pc: u32,
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
        let mut values = Vec::new();
        let mut index = 0;
        while index < literals.len() {
            let key = if object {
                let key = match &literals[index].value {
                    LiteralValue::String(name) => Key::Name(Arc::clone(name)),
                    LiteralValue::Integer(index) => Key::Index(*index),
                    _ => return Err(unsupported(self.pc, "object literal key")),
                };
                index += 1;
                Some(key)
            } else {
                None
            };
            let literal = literals
                .get(index)
                .ok_or_else(|| unsupported(self.pc, "object key without value"))?;
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
                self.value(Op::Closure { method, length }, Effects::READ)
            } else {
                self.literal(literal)?
            };
            if let Some(key) = key {
                entries.push((key, value));
            } else {
                values.push(value);
            }
        }
        Ok(if object {
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
            LiteralValue::Integer(value) => {
                Op::Constant(Constant::Number(f64::from(*value as i32)))
            }
            LiteralValue::Double(bits) => Op::Constant(Constant::Number(f64::from_bits(*bits))),
            LiteralValue::Float(bits) => {
                Op::Constant(Constant::Number(f64::from(f32::from_bits(*bits))))
            }
            LiteralValue::String(name) => Op::Constant(Constant::String(Arc::clone(name))),
            _ => {
                return Err(unsupported(
                    self.pc,
                    format!("literal tag {:#x}", literal.tag),
                ));
            }
        };
        Ok(self.value(op, Effects::PURE))
    }
    fn instruction(&mut self, instruction: &Instruction) -> Result<()> {
        use BinaryOperator as B;
        use UnaryOperator as U;
        self.pc = instruction.pc;
        let name = instruction.opcode.name;
        match name {
            "nop" | "callruntime.notifyconcurrentresult" => {}
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
            "asyncfunctionawaituncaught" => {
                let value = self.acc();
                let result = self.value(Op::Await(value), Effects::SUSPEND);
                self.store(Slot::Resume, result);
            }
            "resumegenerator" => {
                let value = self.read(Slot::Resume);
                self.store(Slot::Acc, value);
            }
            "getresumemode" => self.constant(Constant::Number(0.0)),
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
            "definefunc" | "definemethod" => {
                let method = instruction.entity(1, EntityKind::Method)?;
                self.result(
                    Op::Closure {
                        method,
                        length: if name == "definefunc" {
                            Some(instruction.unsigned(2)?)
                        } else {
                            None
                        },
                    },
                    Effects::CALL,
                );
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
    if !matches!(kind, 0 | 1 | 2 | 4 | 6 | 7) {
        return Err(unsupported(0, format!("function kind {kind}")));
    }
    let asynchronous = matches!(kind, 4 | 6);
    let protocol = if asynchronous {
        Some(super::async_frame::normalize(&mut decoded)?)
    } else {
        None
    };
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
    };
    let mut builder = Builder {
        abc,
        values: Vec::new(),
        slots: HashMap::new(),
        events: Vec::new(),
        block: 0,
        pc: 0,
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
            if !asynchronous
                && matches!(
                    instruction.opcode.name,
                    "asyncfunctionawaituncaught" | "resumegenerator" | "getresumemode"
                )
            {
                return Err(unsupported(
                    instruction.pc,
                    "resume outside verified async frame",
                ));
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
                if !matches!(last.opcode.name, "jeqz" | "jnez") {
                    return Err(unsupported(last.pc, last.opcode.name));
                }
                Terminator::Branch {
                    value: builder.acc(),
                    nonzero: last.opcode.name == "jnez",
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
        } else if terminal {
            return Err(unsupported(last.pc, last.opcode.name));
        } else {
            Terminator::Jump(index + 1)
        };
        let pc = slice[0].pc;
        let mut handlers = Vec::new();
        let mut protected: Vec<_> = decoded
            .code
            .tries
            .iter()
            .filter(|region| region.pc <= pc && pc < region.pc + region.length)
            .collect();
        protected.sort_by_key(|region| region.length);
        for region in protected {
            if region.pc <= pc && pc < region.pc + region.length {
                for handler in &region.catches {
                    if handler.type_index != 0 {
                        return Err(unsupported(pc, "typed exception handler"));
                    }
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
