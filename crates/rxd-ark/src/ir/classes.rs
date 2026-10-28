use crate::{Abc, LiteralValue, Name, reader::invalid};
use rxd_core::Result;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct ClassMember {
    pub name: Arc<Name>,
    pub method: u32,
    pub length: u16,
    pub is_static: bool,
}

pub(crate) fn members(abc: &Abc<'_>, offset: u32) -> Result<Vec<ClassMember>> {
    let literals = abc.literals(offset)?;
    let Some((tail, entries)) = literals.split_last() else {
        return Ok(Vec::new());
    };
    let LiteralValue::Integer(instance_count) = tail.value else {
        return Err(invalid("Ark class literal lacks instance count"));
    };
    if !entries.len().is_multiple_of(3) {
        return Err(super::lift::unsupported(0, "class literal property layout"));
    }
    if instance_count as usize > entries.len() / 3 {
        return Err(invalid("Ark class literal member count mismatch"));
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(entries.len() / 3)
        .map_err(|_| crate::reader::resource("Ark class member allocation failed"))?;
    for (index, triple) in entries.as_chunks::<3>().0.iter().enumerate() {
        let (
            LiteralValue::String(name),
            LiteralValue::Method(method),
            LiteralValue::Affiliate(length),
        ) = (&triple[0].value, &triple[1].value, &triple[2].value)
        else {
            return Err(super::lift::unsupported(0, "class literal member layout"));
        };
        result.push(ClassMember {
            name: Arc::clone(name),
            method: *method,
            length: *length,
            is_static: index >= instance_count as usize,
        });
    }
    Ok(result)
}
