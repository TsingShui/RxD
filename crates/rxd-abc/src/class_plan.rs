use crate::{
    Abc, EntityKind, Instruction, LiteralValue, Method, Name,
    reader::{invalid, resource},
    structure::unsupported,
};
use rxd_core::Result;
use std::sync::Arc;

#[derive(Clone)]
pub enum ClassBase {
    None,
    Register,
}

#[derive(Clone)]
pub struct ClassMember {
    pub name: Arc<Name>,
    pub method: Method,
    pub is_static: bool,
}

#[derive(Clone)]
pub struct ClassPlan {
    pub constructor: Method,
    pub base: ClassBase,
    pub members: Vec<ClassMember>,
}

fn literal_count(value: &LiteralValue) -> Option<u32> {
    match value {
        LiteralValue::Integer(value) => Some(*value),
        LiteralValue::Affiliate(value) => Some(u32::from(*value)),
        _ => None,
    }
}

impl ClassPlan {
    pub fn parse(
        abc: &Abc<'_>,
        owner: &Method,
        instruction: &Instruction,
        proven_hole: bool,
    ) -> Result<Self> {
        if instruction.opcode.name != "defineclasswithbuffer" {
            return Err(unsupported(
                "sendable class construction requires additional reconstruction",
            ));
        }
        let constructor = abc.method(instruction.entity(1, EntityKind::Method)?)?;
        let literals = abc.literals(instruction.entity(2, EntityKind::Literal)?)?;
        let proto = instruction.register(4)?;
        let (last, triples) = literals
            .split_last()
            .ok_or_else(|| invalid("ABC class literal array is empty"))?;
        let method_count = literal_count(&last.value)
            .ok_or_else(|| invalid("ABC class literal array lacks its method count"))?;
        if !literals
            .len()
            .checked_sub(1)
            .is_some_and(|rest| rest % 3 == 0)
        {
            return Err(invalid(
                "ABC class literal array is not name/method/size triples",
            ));
        }
        let entries = (literals.len() - 1) / 3;
        if method_count as usize > entries {
            return Err(invalid("ABC class method count exceeds its literal array"));
        }
        abc.check_storage::<ClassMember>(entries)?;
        let mut members = Vec::new();
        members
            .try_reserve_exact(entries)
            .map_err(|_| resource("ABC class member allocation failed"))?;
        for (index, triple) in triples.as_chunks::<3>().0.iter().enumerate() {
            let LiteralValue::String(name) = &triple[0].value else {
                return Err(invalid("ABC class member name is not a string"));
            };
            let LiteralValue::Method(offset) = triple[1].value else {
                return Err(invalid("ABC class member is not a method"));
            };
            literal_count(&triple[2].value)
                .ok_or_else(|| invalid("ABC class member length is not an integer"))?;
            let method = abc.method(offset)?;
            if method.record != constructor.record {
                return Err(invalid("ABC class member belongs to another record"));
            }
            if method.code_offset.is_none() {
                return Err(invalid("ABC class member has no code item"));
            }
            members.push(ClassMember {
                name: Arc::clone(name),
                method,
                is_static: index >= method_count as usize,
            });
        }
        if constructor.record != owner.record {
            return Err(unsupported("class construction spans ABC records"));
        }
        Ok(Self {
            constructor,
            base: if proven_hole {
                ClassBase::None
            } else {
                let _ = proto;
                ClassBase::Register
            },
            members,
        })
    }
}
