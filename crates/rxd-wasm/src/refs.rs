use anyhow::{Result, ensure};

use crate::model::Reference;
use crate::module::{Module, OpSummary};

/// Find every defined function body that explicitly references `target` via
/// `call`, `return_call`, or `ref.func`. `call_indirect` is never resolved to a
/// concrete callee and is therefore not matched here.
pub fn refs(module: &Module, target: u32) -> Result<Vec<Reference>> {
    let limits = module.limits();
    ensure!(
        target < module.total_functions(),
        "INVALID_QUERY: target function index out of range"
    );
    let mut out = Vec::new();
    let mut work = 0usize;
    for position in 0..module.functions.len() {
        let (_locals, ops) = module.decode_body(position)?;
        work = work.saturating_add(ops.len());
        ensure!(work <= limits.work, "RESOURCE_LIMIT: refs work");
        let function = module.function_at(position);
        for decoded in &ops {
            let (opcode, matched) = match &decoded.op {
                OpSummary::CallDirect { opcode, target: t } if *t == target => (*opcode, true),
                OpSummary::RefFunc { target: t } if *t == target => ("ref.func", true),
                _ => ("", false),
            };
            if matched {
                out.push(Reference {
                    id: function.id.clone(),
                    caller_index: function.func_index,
                    caller_name: function.name.clone(),
                    offset: decoded.code_offset,
                    opcode,
                    target_index: target,
                });
                ensure!(out.len() <= limits.records, "RESOURCE_LIMIT: records");
            }
        }
    }
    Ok(out)
}
