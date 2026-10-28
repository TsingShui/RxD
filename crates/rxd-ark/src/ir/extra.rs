use super::*;

impl Builder<'_, '_> {
    pub(super) fn number(&mut self, value: u32) -> ValueId {
        self.value(
            Op::Constant(Constant::Number(f64::from(value))),
            Effects::PURE,
        )
    }
    pub(super) fn string(&mut self, instruction: &Instruction, index: usize) -> Result<ValueId> {
        Ok(self.value(
            Op::Constant(Constant::String(
                self.abc
                    .name(instruction.entity(index, EntityKind::String)?)?,
            )),
            Effects::PURE,
        ))
    }
    pub(super) fn intrinsic(&mut self, kind: Intrinsic, args: Vec<ValueId>, result: bool) {
        let value = self.value(Op::Intrinsic(kind, args), Effects::CALL);
        if result {
            self.store(Slot::Acc, value);
        }
    }
    pub(super) fn extended(&mut self, i: &Instruction) -> Result<bool> {
        let name = i.opcode.name;
        match name {
            "callruntime.definesendableclass" => {
                let constructor = i.entity(1, EntityKind::Method)?;
                let offset = i.entity(2, EntityKind::Literal)?;
                let members = self.class_members(offset, true)?;
                let base = self.reg(i, 4)?;
                let class = self.value(
                    Op::Class {
                        constructor,
                        name: Arc::clone(&self.abc.method(constructor)?.name),
                        base,
                        environment: None,
                        length: i.unsigned(3)?,
                        members,
                    },
                    Effects::CALL,
                );
                let literals = self.abc.literals(offset)?;
                let metadata = self.literal(
                    literals
                        .last()
                        .ok_or_else(|| invalid("missing sendable field layout"))?,
                )?;
                self.intrinsic(Intrinsic::SendableClass, vec![class, metadata], true);
            }
            "callruntime.newsendableenv" | "callruntime.widenewsendableenv" => {
                let size = self.number(i.unsigned(0)?);
                let hole = self.value(Op::Constant(Constant::Hole), Effects::PURE);
                self.intrinsic(Intrinsic::SendableEnv, vec![size, hole], true);
            }
            "callruntime.ldsendablevar"
            | "callruntime.wideldsendablevar"
            | "callruntime.stsendablevar"
            | "callruntime.widestsendablevar" => {
                let depth = self.number(i.unsigned(0)?);
                let index = self.number(i.unsigned(1)?);
                if name.contains("stsendable") {
                    let value = self.acc();
                    self.intrinsic(Intrinsic::SendableSet, vec![depth, index, value], false);
                } else {
                    self.intrinsic(Intrinsic::SendableGet, vec![depth, index], true);
                }
            }
            "callruntime.ldsendableclass" => {
                let depth = self.number(i.unsigned(0)?);
                self.intrinsic(Intrinsic::SendableLoadClass, vec![depth], true);
            }
            "callruntime.ldsendableexternalmodulevar"
            | "callruntime.wideldsendableexternalmodulevar"
            | "callruntime.ldlazymodulevar"
            | "callruntime.wideldlazymodulevar"
            | "callruntime.ldlazysendablemodulevar"
            | "callruntime.wideldlazysendablemodulevar" => {
                let value = self.value(Op::Import(i.unsigned(0)?), Effects::READ);
                self.intrinsic(
                    if name.contains("lazy") {
                        Intrinsic::LazyImport
                    } else {
                        Intrinsic::SendableImport
                    },
                    vec![value],
                    true,
                );
            }
            "callruntime.notifyconcurrentresult" => {
                let value = self.acc();
                self.intrinsic(Intrinsic::ConcurrentResult, vec![value], false);
            }
            "ldbigint" => {
                let text = self.string(i, 0)?;
                self.intrinsic(Intrinsic::BigInt, vec![text], true);
            }
            "createregexpwithliteral" => {
                let text = self.string(i, 1)?;
                let flags = self.number(i.unsigned(2)?);
                self.intrinsic(Intrinsic::RegExp, vec![text, flags], true);
            }
            "ldsymbol" => self.intrinsic(Intrinsic::Symbol, vec![], true),
            "getunmappedargs" => self.intrinsic(Intrinsic::Arguments, vec![], true),
            "debugger" => self.action(Action::Debugger),
            "ldthisbyname" | "ldthisbyvalue" => {
                let object = self.value(Op::This, Effects::PURE);
                let key = if name == "ldthisbyname" {
                    self.key(i, 1)?
                } else {
                    Key::Value(self.acc())
                };
                self.result(Op::Get(object, key), Effects::READ);
            }
            "stthisbyname" | "stthisbyvalue" => {
                let object = self.value(Op::This, Effects::PURE);
                let key = if name == "stthisbyname" {
                    self.key(i, 1)?
                } else {
                    Key::Value(self.reg(i, 1)?)
                };
                let value = self.acc();
                self.action(Action::Set {
                    object,
                    key,
                    value,
                    own: StoreKind::Assign,
                });
            }
            "callruntime.topropertykey" => {
                let value = self.acc();
                self.intrinsic(Intrinsic::PropertyKey, vec![value], true);
            }
            "setobjectwithproto" => {
                let proto = self.reg(i, 1)?;
                let object = self.acc();
                self.intrinsic(Intrinsic::SetPrototype, vec![object, proto], false);
            }
            "createobjectwithexcludedkeys" | "wide.createobjectwithexcludedkeys" => {
                let source = self.reg(i, 1)?;
                let keys = self.range(
                    i.register(2)?,
                    i.unsigned(0)?
                        .checked_add(1)
                        .ok_or_else(|| invalid("Ark excluded-key range overflow"))?,
                )?;
                let keys = self.value(Op::Array(keys), Effects::READ);
                self.intrinsic(Intrinsic::ObjectRest, vec![source, keys], true);
            }
            "newobjapply" => {
                let constructor = self.acc();
                let args = self.reg(i, 1)?;
                self.intrinsic(Intrinsic::ConstructSpread, vec![constructor, args], true);
            }
            "closeiterator" => {
                let iterator = self.reg(i, 1)?;
                self.intrinsic(Intrinsic::CloseIterator, vec![iterator], true);
            }
            "getasynciterator" => {
                let object = self.acc();
                self.intrinsic(Intrinsic::AsyncIterator, vec![object], true);
            }
            "gettemplateobject" => {
                let object = self.acc();
                self.intrinsic(Intrinsic::Template, vec![object], true);
            }
            "callruntime.callinit" => {
                let callee = self.acc();
                let receiver = Some(self.reg(i, 1)?);
                self.value(
                    Op::Call {
                        callee,
                        receiver,
                        args: vec![],
                    },
                    Effects::CALL,
                );
            }
            "ldprivateproperty"
            | "stprivateproperty"
            | "testin"
            | "callruntime.defineprivateproperty" => {
                let key = self.value(
                    Op::LoadCell {
                        depth: i.unsigned(1)?,
                        index: i.unsigned(2)?,
                    },
                    Effects::READ,
                );
                let (kind, args, result) = match name {
                    "ldprivateproperty" => (Intrinsic::PrivateGet, vec![key, self.acc()], true),
                    "testin" => (Intrinsic::PrivateHas, vec![key, self.acc()], true),
                    _ => (
                        if name == "stprivateproperty" {
                            Intrinsic::PrivateSet
                        } else {
                            Intrinsic::PrivateDefine
                        },
                        vec![key, self.reg(i, 3)?, self.acc()],
                        false,
                    ),
                };
                self.intrinsic(kind, args, result);
            }
            "callruntime.createprivateproperty" => {
                let env = self.value(Op::Environment, Effects::READ);
                let count = self.number(i.unsigned(0)?);
                let literal = self.buffer(i, false)?;
                let literal = self.value(literal, Effects::READ);
                let hole = self.value(Op::Constant(Constant::Hole), Effects::PURE);
                self.intrinsic(
                    Intrinsic::PrivateKeys,
                    vec![env, count, literal, hole],
                    false,
                );
            }
            "ldglobalvar" => {
                let key = self.string(i, 1)?;
                self.intrinsic(Intrinsic::GlobalGet, vec![key], true);
            }
            "trystglobalbyname" | "stglobalvar" => {
                let key = self.string(i, 1)?;
                let value = self.acc();
                let strict = self.value(
                    Op::Constant(Constant::Bool(name == "trystglobalbyname")),
                    Effects::PURE,
                );
                self.intrinsic(Intrinsic::GlobalSet, vec![key, value, strict], false);
            }
            "stconsttoglobalrecord" | "sttoglobalrecord" => {
                let key = self.string(i, 1)?;
                let value = self.acc();
                let constant = self.value(
                    Op::Constant(Constant::Bool(name == "stconsttoglobalrecord")),
                    Effects::PURE,
                );
                self.intrinsic(Intrinsic::GlobalDeclare, vec![key, value, constant], false);
            }
            "wide.ldpatchvar" => {
                let index = self.number(i.unsigned(0)?);
                self.intrinsic(Intrinsic::PatchGet, vec![index], true);
            }
            "wide.stpatchvar" => {
                let index = self.number(i.unsigned(0)?);
                let value = self.acc();
                self.intrinsic(Intrinsic::PatchSet, vec![index, value], false);
            }
            "stownbynamewithnameset" | "stownbyvaluewithnameset" => {
                let (target, key) = if name == "stownbynamewithnameset" {
                    (self.reg(i, 2)?, self.string(i, 1)?)
                } else {
                    (self.reg(i, 1)?, self.reg(i, 2)?)
                };
                let value = self.acc();
                self.intrinsic(Intrinsic::OwnNamed, vec![target, key, value], false);
            }
            "throw.undefinedifhole" => {
                let value = self.reg(i, 0)?;
                let name = self.reg(i, 1)?;
                self.action(Action::CheckHoleValue(value, name));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub(super) fn branch_compare(&mut self, i: &Instruction) -> Result<ValueId> {
        use BinaryOperator as B;
        let name = i.opcode.name;
        let operator = match name {
            "jeqnull" | "jequndefined" | "jeq" => B::Equality,
            "jnenull" | "jneundefined" | "jne" => B::Inequality,
            "jstricteqz" | "jstricteqnull" | "jstrictequndefined" | "jstricteq" => {
                B::StrictEquality
            }
            "jnstricteqz" | "jnstricteqnull" | "jnstrictequndefined" | "jnstricteq" => {
                B::StrictInequality
            }
            _ => return Err(invalid("unknown Ark conditional opcode")),
        };
        let left = self.acc();
        let right = if name.ends_with("null") {
            self.value(Op::Constant(Constant::Null), Effects::PURE)
        } else if name.ends_with("undefined") {
            self.value(Op::Constant(Constant::Undefined), Effects::PURE)
        } else if name.ends_with('z') {
            self.number(0)
        } else {
            self.reg(i, 0)?
        };
        Ok(self.value(Op::BranchCompare(operator, left, right), Effects::CALL))
    }
}
