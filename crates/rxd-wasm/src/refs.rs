use anyhow::{Result, ensure};
use std::collections::HashSet;

use crate::model::Reference;
use crate::module::{Module, OpSummary};

/// Find every explicit instruction reference to the selected targets in one scan via
/// `call`, `return_call`, or `ref.func`. `call_indirect` is never resolved to a
/// concrete callee and is therefore not matched here.
pub fn refs(module: &Module, targets: &[u32]) -> Result<Vec<Reference>> {
    let limits = module.limits();
    ensure!(
        limits.work.is_none_or(|limit| targets.len() <= limit),
        rxd_core::Error::failure(rxd_core::Failure::Resource, "reference targets")
    );
    for target in targets {
        ensure!(
            *target < module.total_functions(),
            rxd_core::Error::failure(
                rxd_core::Failure::NotFound,
                format!("no function func:{target} in this module")
            )
        );
    }
    let targets: HashSet<u32> = targets.iter().copied().collect();
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut work = 0usize;
    let mut retained = 0usize;
    let metadata_limit = limits.metadata_bytes;
    for position in 0..module.functions.len() {
        let (_locals, ops) = module.decode_body(position)?;
        work = work.saturating_add(ops.len());
        ensure!(
            limits.work.is_none_or(|limit| work <= limit),
            "RESOURCE_LIMIT: refs work"
        );
        let function = module.function_at(position);
        for decoded in &ops {
            let (opcode, target) = match &decoded.op {
                OpSummary::CallDirect { opcode, target } if targets.contains(target) => {
                    (*opcode, *target)
                }
                OpSummary::RefFunc { target } if targets.contains(target) => ("ref.func", *target),
                _ => continue,
            };
            retained = retained
                .saturating_add(std::mem::size_of::<Reference>())
                .saturating_add(function.id.len())
                .saturating_add(function.name.as_ref().map_or(0, String::len));
            ensure!(
                retained <= metadata_limit,
                rxd_core::Error::failure(rxd_core::Failure::Resource, "reference metadata bytes")
            );
            out.push(Reference {
                id: function.id.clone(),
                caller_index: function.func_index,
                caller_name: function.name.clone(),
                offset: decoded.code_offset,
                opcode,
                target_index: target,
            });
        }
    }
    Ok(out)
}
