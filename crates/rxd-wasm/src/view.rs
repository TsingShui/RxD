use anyhow::{Result, ensure};
use wasmparser::{BinaryReader, FunctionBody, Operator};

use crate::model::{FunctionView, Instruction, LocalDeclView, MemArg, Operand};
use crate::module::Module;

/// Decode original function instructions; unsupported opcodes retain raw body evidence.
pub fn instruction_view(module: &Module, position: usize) -> Result<FunctionView> {
    let function = module.function_at(position).clone();
    let limits = module.limits();
    let (range, bytes) = module.body_slice(position);
    let body = FunctionBody::new(BinaryReader::new(bytes, range.start));

    let mut locals = Vec::new();
    for local in body
        .get_locals_reader()
        .map_err(|e| anyhow::anyhow!("INVALID_INPUT: locals: {e}"))?
    {
        let (count, ty) = local.map_err(|e| anyhow::anyhow!("INVALID_INPUT: locals: {e}"))?;
        locals.push(LocalDeclView {
            count,
            value_type: crate::module::valtype_display(&ty),
        });
    }

    let base = range.start;
    let mut reader = body
        .get_operators_reader()
        .map_err(|e| anyhow::anyhow!("INVALID_INPUT: operators: {e}"))?;

    let mut instructions = Vec::new();
    let mut reason = None;
    let mut budget = 0usize;
    while !reader.eof() {
        let (op, pos) = reader
            .read_with_offset()
            .map_err(|e| anyhow::anyhow!("INVALID_INPUT: operator decode: {e}"))?;
        budget += 1;
        ensure!(
            limits.instructions.is_none_or(|limit| budget <= limit),
            "RESOURCE_LIMIT: per-function instructions"
        );
        let mnemonic = mnemonic(&op);
        if !is_covered(&op) {
            reason = Some(format!("UNSUPPORTED_OPCODE: {mnemonic}"));
            break;
        }
        instructions.push(Instruction {
            offset: pos - base,
            opcode: mnemonic.to_string(),
            operand: operand(&op),
        });
    }

    let decompiled = false;
    Ok(FunctionView {
        representation: "wasm",
        decompiled,
        reason,
        code: None,
        func_index: function.func_index,
        type_index: function.type_index,
        name: function.name,
        params: function.params,
        results: function.results,
        locals,
        body_file_offset: function.body_file_offset,
        body_size: function.body_size,
        bytecode_hex: module.body_hex(position),
        instructions,
        mapping: Vec::new(),
    })
}

pub fn op_mnemonic(op: &Operator) -> &'static str {
    mnemonic(op)
}

/// Covered = MVP plus stabilized, widely deployed proposals. Everything else
/// (SIMD, threads/atomics, GC, exceptions, experimental) is reported uncovered.
fn is_covered(op: &Operator) -> bool {
    macro_rules! classify {
        ($( @$proposal:ident $name:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
            match op {
                $(
                    Operator::$name $( { $($arg),* } )? => {
                        $( $( let _ = $arg; )* )?
                        classify!(covered @$proposal)
                    }
                )*
                _ => false,
            }
        };
        (covered @mvp) => { true };
        (covered @sign_extension) => { true };
        (covered @saturating_float_to_int) => { true };
        (covered @bulk_memory) => { true };
        (covered @reference_types) => { true };
        (covered @tail_call) => { true };
        (covered @multi_value) => { true };
        (covered @$other:ident) => { false };
    }
    wasmparser::for_each_operator!(classify)
}

fn mnemonic(op: &Operator) -> &'static str {
    macro_rules! name {
        ($( @$proposal:ident $opname:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*))*) => {
            match op {
                $(
                    Operator::$opname $( { $($arg),* } )? => {
                        $( $( let _ = $arg; )* )?
                        stringify_mnemonic(stringify!($visit))
                    }
                )*
                _ => "unknown",
            }
        };
    }
    wasmparser::for_each_operator!(name)
}

/// Convert visitor names to mnemonics, e.g. visit_i32_add -> i32.add.
fn stringify_mnemonic(visit: &'static str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap();
    if let Some(found) = map.get(visit) {
        return found;
    }
    let base = visit.strip_prefix("visit_").unwrap_or(visit);
    let dotted = with_dot(base);
    let leaked: &'static str = Box::leak(dotted.into_boxed_str());
    map.insert(visit, leaked);
    leaked
}

fn with_dot(base: &str) -> String {
    // Leading namespace tokens that take a `.` separator in text mnemonics.
    const NS: &[&str] = &[
        "i32", "i64", "f32", "f64", "v128", "local", "global", "memory", "table", "elem", "data",
        "ref",
    ];
    for ns in NS {
        if let Some(rest) = base.strip_prefix(ns)
            && let Some(tail) = rest.strip_prefix('_')
        {
            return format!("{ns}.{tail}");
        }
    }
    base.to_string()
}

fn operand(op: &Operator) -> Operand {
    match op {
        Operator::I32Const { value } => Operand::I32 { value: *value },
        Operator::I64Const { value } => Operand::I64 { value: *value },
        Operator::F32Const { value } => Operand::F32 { bits: value.bits() },
        Operator::F64Const { value } => Operand::F64 { bits: value.bits() },
        Operator::LocalGet { local_index }
        | Operator::LocalSet { local_index }
        | Operator::LocalTee { local_index } => Operand::Local {
            index: *local_index,
        },
        Operator::GlobalGet { global_index } | Operator::GlobalSet { global_index } => {
            Operand::Global {
                index: *global_index,
            }
        }
        Operator::Call { function_index }
        | Operator::ReturnCall { function_index }
        | Operator::RefFunc { function_index } => Operand::Function {
            index: *function_index,
        },
        Operator::CallIndirect {
            type_index,
            table_index,
        }
        | Operator::ReturnCallIndirect {
            type_index,
            table_index,
        } => Operand::CallIndirect {
            type_index: *type_index,
            table_index: *table_index,
        },
        Operator::Br { relative_depth } | Operator::BrIf { relative_depth } => Operand::Branch {
            depth: *relative_depth,
        },
        Operator::BrTable { targets } => {
            let default = targets.default();
            let list: Vec<u32> = targets.targets().filter_map(|t| t.ok()).collect();
            Operand::BranchTable {
                targets: list,
                default,
            }
        }
        Operator::Block { blockty } | Operator::Loop { blockty } | Operator::If { blockty } => {
            Operand::Block {
                block_type: format!("{blockty:?}"),
            }
        }
        _ => memory_operand(op).unwrap_or(Operand::None),
    }
}

fn memory_operand(op: &Operator) -> Option<Operand> {
    let memarg = match op {
        Operator::I32Load { memarg }
        | Operator::I64Load { memarg }
        | Operator::F32Load { memarg }
        | Operator::F64Load { memarg }
        | Operator::I32Load8S { memarg }
        | Operator::I32Load8U { memarg }
        | Operator::I32Load16S { memarg }
        | Operator::I32Load16U { memarg }
        | Operator::I64Load8S { memarg }
        | Operator::I64Load8U { memarg }
        | Operator::I64Load16S { memarg }
        | Operator::I64Load16U { memarg }
        | Operator::I64Load32S { memarg }
        | Operator::I64Load32U { memarg }
        | Operator::I32Store { memarg }
        | Operator::I64Store { memarg }
        | Operator::F32Store { memarg }
        | Operator::F64Store { memarg }
        | Operator::I32Store8 { memarg }
        | Operator::I32Store16 { memarg }
        | Operator::I64Store8 { memarg }
        | Operator::I64Store16 { memarg }
        | Operator::I64Store32 { memarg } => memarg,
        _ => return None,
    };
    Some(Operand::Memory {
        memarg: MemArg {
            align: memarg.align,
            offset: memarg.offset,
            memory: memarg.memory,
        },
    })
}
