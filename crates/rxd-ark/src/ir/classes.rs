use super::{Key, ValueId};
use crate::{Abc, Literal, LiteralValue, reader::invalid};
use rxd_core::Result;

#[derive(Clone, Debug)]
pub(crate) struct ClassMember {
    pub name: Key,
    pub method: Option<u32>,
    pub value: Option<ValueId>,
    pub length: u16,
    pub is_static: bool,
    pub accessor: bool,
}
pub(super) struct Member {
    pub key: Literal,
    pub value: Literal,
    pub length: u16,
    pub is_static: bool,
}
pub(super) fn members(abc: &Abc<'_>, offset: u32, sendable: bool) -> Result<Vec<Member>> {
    let literals = abc.literals(offset)?;
    let literals = if sendable {
        literals
            .get(..literals.len().saturating_sub(1))
            .ok_or_else(|| invalid("missing sendable field metadata"))?
    } else {
        &literals
    };
    let Some((tail, entries)) = literals.split_last() else {
        return Ok(vec![]);
    };
    let LiteralValue::Integer(instance_count) = tail.value else {
        return Err(invalid("Ark class literal lacks instance count"));
    };
    let mut result = Vec::new();
    result
        .try_reserve(entries.len() / 2)
        .map_err(|_| crate::reader::resource("Ark class member allocation failed"))?;
    let mut index = 0;
    while index < entries.len() {
        let key = entries[index].clone();
        index += 1;
        let value = entries
            .get(index)
            .ok_or_else(|| invalid("Ark class property lacks a value"))?
            .clone();
        index += 1;
        let length = if matches!(value.value, LiteralValue::Method(_)) {
            let Some(Literal {
                value: LiteralValue::Affiliate(length),
                ..
            }) = entries.get(index)
            else {
                return Err(invalid("Ark class method lacks its affiliate length"));
            };
            index += 1;
            *length
        } else {
            0
        };
        result.push(Member {
            key,
            value,
            length,
            is_static: result.len() >= instance_count as usize,
        });
    }
    if instance_count as usize > result.len() {
        return Err(invalid("Ark class instance count exceeds properties"));
    }
    Ok(result)
}
