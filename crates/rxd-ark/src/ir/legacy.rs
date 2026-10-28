use super::*;

impl Builder<'_, '_> {
    fn literal_operand(&self, index: u32) -> Result<Operand> {
        let offset = self.abc.resolve_entity(self.method, index)?;
        self.abc.literals(offset)?;
        Ok(Operand::Id {
            kind: EntityKind::Literal,
            index,
            offset,
        })
    }
    pub(super) fn legacy(&mut self, i: &Instruction) -> Result<bool> {
        let Some(name) = i.opcode.name.strip_prefix("deprecated.") else {
            return Ok(false);
        };
        let saved_acc = (!i.opcode.writes_acc()).then(|| self.acc());
        let saved_env = self.definition_env;
        let zero = Operand::Unsigned(0);
        let mut args = i.operands.clone();
        let target = match name {
            "ldlexenv" => {
                self.result(Op::Environment, Effects::READ);
                return Ok(true);
            }
            "ldhomeobject" => {
                self.result(Op::Home, Effects::READ);
                return Ok(true);
            }
            "poplexenv" => {
                self.action(Action::PopEnv);
                self.result(Op::Environment, Effects::READ);
                return Ok(true);
            }
            "getiteratornext" => {
                let iterator = self.reg(i, 0)?;
                let method = self.reg(i, 1)?;
                self.intrinsic(Intrinsic::IteratorNext, vec![iterator, method], true);
                return Ok(true);
            }
            "neg" | "not" | "inc" | "dec" | "tonumber" | "tonumeric" | "gettemplateobject" => {
                let value = self.reg(i, 0)?;
                self.store(Slot::Acc, value);
                args = vec![zero];
                name
            }
            "dynamicimport" | "resumegenerator" | "getresumemode" => {
                let value = self.reg(i, 0)?;
                self.store(Slot::Acc, value);
                args = vec![];
                name
            }
            "callarg0" | "callarg1" | "callargs2" | "callargs3" => {
                let callee = self.reg(i, 0)?;
                self.store(Slot::Acc, callee);
                args[0] = zero;
                name
            }
            "callrange" | "callthisrange" => {
                let start = i.register(1)?;
                let count = i.unsigned(0)?;
                let callee = self.read(Slot::Register(start));
                let receiver = if name == "callthisrange" {
                    Some(self.read(Slot::Register(start + 1)))
                } else {
                    None
                };
                let count = count
                    .checked_sub(u32::from(receiver.is_some()))
                    .ok_or_else(|| invalid("legacy callthisrange lacks receiver"))?;
                let args = self.range(start + 1 + u32::from(receiver.is_some()), count)?;
                self.result(
                    Op::Call {
                        callee,
                        receiver,
                        args,
                    },
                    Effects::CALL,
                );
                return Ok(true);
            }
            "callspread" => {
                let function = self.reg(i, 0)?;
                let receiver = self.reg(i, 1)?;
                let args = self.reg(i, 2)?;
                self.result(Op::Apply(function, receiver, args), Effects::CALL);
                return Ok(true);
            }
            "createarraywithbuffer" | "createobjectwithbuffer" | "createobjecthavingmethod" => {
                args = vec![zero, self.literal_operand(i.unsigned(0)?)?];
                if name == "createobjecthavingmethod" {
                    self.definition_env = Some(self.acc());
                    "createobjectwithbuffer"
                } else {
                    name
                }
            }
            "defineclasswithbuffer" => {
                let method_index = match &i.operands[0] {
                    Operand::Id { index, .. } => *index,
                    _ => return Err(invalid("legacy class method operand")),
                };
                args = vec![
                    zero,
                    i.operands[0].clone(),
                    self.literal_operand(
                        method_index
                            .checked_add(1)
                            .ok_or_else(|| invalid("legacy literal index overflow"))?,
                    )?,
                    i.operands[2].clone(),
                    i.operands[4].clone(),
                ];
                // Legacy classes carry an explicit lexical environment register.
                self.definition_env = Some(self.reg(i, 3)?);
                name
            }
            "delobjprop" | "copydataproperties" => {
                let value = self.reg(i, 1)?;
                self.store(Slot::Acc, value);
                args = vec![i.operands[0].clone()];
                name
            }
            "setobjectwithproto" => {
                let value = self.reg(i, 1)?;
                self.store(Slot::Acc, value);
                args = vec![zero, i.operands[0].clone()];
                name
            }
            "ldobjbyvalue" | "ldsuperbyvalue" => {
                let value = self.reg(i, 1)?;
                self.store(Slot::Acc, value);
                args = vec![zero, i.operands[0].clone()];
                name
            }
            "ldobjbyindex" => {
                let value = self.reg(i, 0)?;
                self.store(Slot::Acc, value);
                args = vec![zero, i.operands[1].clone()];
                name
            }
            "ldobjbyname" | "ldsuperbyname" => {
                let value = self.reg(i, 1)?;
                self.store(Slot::Acc, value);
                args = vec![zero, i.operands[0].clone()];
                name
            }
            "stlexvar" => {
                let value = self.reg(i, 2)?;
                self.store(Slot::Acc, value);
                args.truncate(2);
                name
            }
            "suspendgenerator" | "asyncfunctionawaituncaught" => {
                let value = self.reg(i, 1)?;
                self.store(Slot::Acc, value);
                args.truncate(1);
                name
            }
            "asyncfunctionresolve" | "asyncfunctionreject" => {
                let value = self.reg(i, 2)?;
                self.store(Slot::Acc, value);
                args.truncate(1);
                name
            }
            "asyncgeneratorreject" => {
                let value = self.reg(i, 1)?;
                self.store(Slot::Acc, value);
                args.truncate(1);
                name
            }
            "stconsttoglobalrecord" | "stlettoglobalrecord" | "stclasstoglobalrecord" => {
                args.insert(0, zero);
                if name == "stconsttoglobalrecord" {
                    name
                } else {
                    "sttoglobalrecord"
                }
            }
            "getmodulenamespace" | "stmodulevar" | "ldmodulevar" => {
                let key = self.abc.name(i.entity(0, EntityKind::String)?)?;
                let module = self
                    .abc
                    .module(&self.abc.record(self.method.record)?)?
                    .ok_or_else(|| invalid("legacy module operation lacks module metadata"))?;
                if name == "getmodulenamespace" {
                    let index = module
                        .requests
                        .iter()
                        .position(|request| request.units == key.units)
                        .ok_or_else(|| invalid("legacy module request is not declared"))?;
                    self.result(Op::Namespace(index as u32), Effects::READ);
                } else if name == "stmodulevar" || i.unsigned(1)? != 0 {
                    let index = module
                        .local_exports
                        .iter()
                        .position(|export| export.local_name.units == key.units)
                        .ok_or_else(|| invalid("legacy local module binding is not declared"))?;
                    if name == "stmodulevar" {
                        let value = self.acc();
                        self.action(Action::StoreModule(index as u32, value));
                    } else {
                        self.result(Op::Module(index as u32), Effects::READ);
                    }
                } else {
                    let index = module
                        .regular_imports
                        .iter()
                        .position(|import| import.local_name.units == key.units)
                        .ok_or_else(|| invalid("legacy imported binding is not declared"))?;
                    self.result(Op::Import(index as u32), Effects::READ);
                }
                return Ok(true);
            }
            _ => return Ok(false),
        };
        self.instruction(&i.semantic_alias(target, args)?)?;
        self.definition_env = saved_env;
        if let Some(acc) = saved_acc {
            self.store(Slot::Acc, acc);
        }
        Ok(true)
    }
}
