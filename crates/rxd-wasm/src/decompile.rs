//! Structured, language-neutral C-like decompilation of a single function.
//!
//! This is rxd's own stack-reconstruction and control-flow mapping, built on
//! WASM's native structured control flow. It is NOT a source-language recovery
//! and NOT deobfuscation. Any uncovered opcode, non-structured situation, or
//! stack underflow fully degrades to the instruction view: the caller falls
//! back and never emits partial or guessed code.

use anyhow::Result;
use wasmparser::{BinaryReader, BlockType, FunctionBody, Operator};

use crate::model::FunctionView;
use crate::module::{Module, valtype_display};

/// Outcome of a decompilation attempt.
pub enum Attempt {
    /// Reconstructed C-like code with a line -> code-body-offset mapping.
    Decompiled {
        code: String,
        mapping: Vec<(usize, usize)>,
    },
    /// Could not safely reconstruct; the caller must use the instruction view.
    Degraded { reason: String },
}

/// Try to decompile one defined function. Pure analysis over a bounded body.
pub fn decompile(module: &Module, position: usize) -> Result<Attempt> {
    let function = module.function_at(position);
    let (range, bytes) = module.body_slice(position);
    let body = FunctionBody::new(BinaryReader::new(bytes, range.start));

    // Local names: params a, b, c…, then declared locals continue the sequence.
    let mut local_types = Vec::new();
    for ty in &function.params {
        local_types.push(ty.clone());
    }
    for local in body
        .get_locals_reader()
        .map_err(|e| anyhow::anyhow!("INVALID_INPUT: locals: {e}"))?
    {
        let (count, ty) = local.map_err(|e| anyhow::anyhow!("INVALID_INPUT: locals: {e}"))?;
        for _ in 0..count {
            local_types.push(valtype_display(&ty));
        }
    }

    let base = range.start;
    let mut reader = body
        .get_operators_reader()
        .map_err(|e| anyhow::anyhow!("INVALID_INPUT: operators: {e}"))?;

    let mut d = Decompiler::new(module, &local_types, function.results.len());
    let limit = module.limits().instructions;
    let mut budget = 0usize;
    while !reader.eof() {
        let (op, pos) = reader
            .read_with_offset()
            .map_err(|e| anyhow::anyhow!("INVALID_INPUT: operator decode: {e}"))?;
        budget += 1;
        if limit.is_some_and(|limit| budget > limit) {
            anyhow::bail!("RESOURCE_LIMIT: per-function instructions");
        }
        match d.step(&op, pos - base) {
            Ok(()) => {}
            Err(reason) => return Ok(Attempt::Degraded { reason }),
        }
    }
    Ok(d.finish())
}

const P_ATOM: u8 = 100;
const P_INDEX: u8 = 90;
const P_MUL: u8 = 70;
const P_ADD: u8 = 60;
const P_SHIFT: u8 = 50;
const P_CMP: u8 = 40;
const P_BITAND: u8 = 32;
const P_BITXOR: u8 = 30;
const P_BITOR: u8 = 28;

#[derive(Clone)]
struct Value {
    text: String,
    prec: u8,
}

enum FrameKind {
    Block,
    Loop,
    If,
}

struct Frame {
    kind: FrameKind,
    label: String,
    used_as_break: bool,
    lines: Vec<Line>,
    // For If frames: the then-part is `lines`, else-part accumulates here.
    else_lines: Option<Vec<Line>>,
    in_else: bool,
}

#[derive(Clone)]
struct Line {
    text: String,
    offset: usize,
}

struct Decompiler<'a> {
    module: &'a Module,
    local_types: &'a [String],
    func_results: usize,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    declared: std::collections::HashSet<usize>,
    label_counter: usize,
    temp_counter: usize,
}

impl<'a> Decompiler<'a> {
    fn new(module: &'a Module, local_types: &'a [String], func_results: usize) -> Self {
        Decompiler {
            module,
            local_types,
            func_results,
            stack: Vec::new(),
            frames: vec![Frame {
                kind: FrameKind::Block,
                label: "FUNC".into(),
                used_as_break: false,
                lines: Vec::new(),
                else_lines: None,
                in_else: false,
            }],
            declared: std::collections::HashSet::new(),
            label_counter: 0,
            temp_counter: 0,
        }
    }

    fn push(&mut self, text: String, prec: u8) {
        self.stack.push(Value { text, prec });
    }

    fn pop(&mut self) -> std::result::Result<Value, String> {
        self.stack
            .pop()
            .ok_or_else(|| "UNSUPPORTED_STACK: operand stack underflow".to_string())
    }

    fn emit(&mut self, text: String, offset: usize) {
        let frame = self.frames.last_mut().unwrap();
        let line = Line { text, offset };
        if frame.in_else {
            frame.else_lines.get_or_insert_with(Vec::new).push(line);
        } else {
            frame.lines.push(line);
        }
    }

    fn local_name(&self, index: u32) -> String {
        let i = index as usize;
        if i < 26 {
            ((b'a' + i as u8) as char).to_string()
        } else {
            format!("v{i}")
        }
    }

    fn new_label(&mut self) -> String {
        let l = format!("L{}", self.label_counter);
        self.label_counter += 1;
        l
    }

    fn wrap(v: &Value, parent: u8) -> String {
        if v.prec < parent {
            format!("({})", v.text)
        } else {
            v.text.clone()
        }
    }

    fn bin(&mut self, op: &str, prec: u8) -> std::result::Result<(), String> {
        let b = self.pop()?;
        let a = self.pop()?;
        let text = format!("{} {op} {}", Self::wrap(&a, prec), Self::wrap(&b, prec));
        self.push(text, prec);
        Ok(())
    }

    fn unary(&mut self, f: &str) -> std::result::Result<(), String> {
        let a = self.pop()?;
        self.push(format!("{f}({})", a.text), P_ATOM);
        Ok(())
    }

    /// A value-preserving cast rendered as `(ty)expr`.
    fn cast(&mut self, ty: &str) -> std::result::Result<(), String> {
        let a = self.pop()?;
        self.push(format!("({ty}){}", Self::wrap(&a, P_ATOM)), P_ATOM);
        Ok(())
    }

    /// A binary operation rendered as a function call, e.g. `rotr(a, b)`.
    fn binfn(&mut self, f: &str) -> std::result::Result<(), String> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.push(format!("{f}({}, {})", a.text, b.text), P_ATOM);
        Ok(())
    }

    fn load(&mut self, ty: &str, offset: u64) -> std::result::Result<(), String> {
        let a = self.pop()?;
        let base = Self::wrap(&a, P_INDEX);
        self.push(Self::mem_access(&base, ty, offset), P_INDEX);
        Ok(())
    }

    fn store(&mut self, ty: &str, offset: u64, at: usize) -> std::result::Result<(), String> {
        let value = self.pop()?;
        let addr = self.pop()?;
        let base = Self::wrap(&addr, P_INDEX);
        let lhs = Self::mem_access(&base, ty, offset);
        self.emit(format!("{lhs} = {};", value.text), at);
        Ok(())
    }

    /// Render a memory access as array indexing, scaling the static byte offset
    /// by the element size when it divides evenly (mirrors wasm-decompile:
    /// `a[12]:int` -> `a[3]:int`). Also folds a scaled dynamic index
    /// `(base + (i << k))[off]:T` -> `base[i]:T` (or `base[i + n]:T` when a
    /// non-zero aligned static offset `n*size` remains) when `2^k` is the
    /// element size. Non-aligned offsets keep the raw byte index.
    fn mem_access(base: &str, ty: &str, offset: u64) -> String {
        let size = type_byte_size(ty);
        if size > 1
            && offset.is_multiple_of(size)
            && let Some((outer, idx)) = split_scaled_index(base, size)
        {
            let scaled = offset / size;
            return if scaled == 0 {
                format!("{outer}[{idx}]:{ty}")
            } else {
                format!("{outer}[{idx} + {scaled}]:{ty}")
            };
        }
        if size > 1 && offset.is_multiple_of(size) {
            format!("{base}[{}]:{ty}", offset / size)
        } else {
            format!("{base}[{offset}]:{ty}")
        }
    }

    /// Resolve a relative branch depth to a target frame. Returns (label, is_loop).
    fn branch_target(&mut self, rel: u32) -> std::result::Result<(String, bool), String> {
        if rel as usize >= self.frames.len() {
            return Err("UNSUPPORTED_BRANCH: relative depth out of range".into());
        }
        let idx = self.frames.len() - 1 - rel as usize;
        let is_loop = matches!(self.frames[idx].kind, FrameKind::Loop);
        if !is_loop {
            self.frames[idx].used_as_break = true;
        }
        Ok((self.frames[idx].label.clone(), is_loop))
    }

    fn step(&mut self, op: &Operator, at: usize) -> std::result::Result<(), String> {
        match op {
            // ---- constants ----
            Operator::I32Const { value } => self.push(value.to_string(), P_ATOM),
            Operator::I64Const { value } => self.push(format!("{value}L"), P_ATOM),
            Operator::F32Const { value } => {
                self.push(format!("{}f", f32::from_bits(value.bits())), P_ATOM)
            }
            Operator::F64Const { value } => {
                self.push(format!("{}", f64::from_bits(value.bits())), P_ATOM)
            }

            // ---- locals / globals ----
            Operator::LocalGet { local_index } => self.push(self.local_name(*local_index), P_ATOM),
            Operator::LocalSet { local_index } => {
                let v = self.pop()?;
                let line = self.assign_local(*local_index, &v.text);
                self.emit(line, at);
            }
            Operator::LocalTee { local_index } => {
                let v = self.pop()?;
                let line = self.assign_local(*local_index, &v.text);
                self.emit(line, at);
                self.push(self.local_name(*local_index), P_ATOM);
            }
            Operator::GlobalGet { global_index } => self.push(format!("g{global_index}"), P_ATOM),
            Operator::GlobalSet { global_index } => {
                let v = self.pop()?;
                self.emit(format!("g{global_index} = {};", v.text), at);
            }

            // ---- integer / float arithmetic & comparison ----
            Operator::I32Add | Operator::I64Add | Operator::F32Add | Operator::F64Add => {
                self.bin("+", P_ADD)?
            }
            Operator::I32Sub | Operator::I64Sub | Operator::F32Sub | Operator::F64Sub => {
                self.bin("-", P_ADD)?
            }
            Operator::I32Mul | Operator::I64Mul | Operator::F32Mul | Operator::F64Mul => {
                self.bin("*", P_MUL)?
            }
            Operator::I32DivS
            | Operator::I32DivU
            | Operator::I64DivS
            | Operator::I64DivU
            | Operator::F32Div
            | Operator::F64Div => self.bin("/", P_MUL)?,
            Operator::I32RemS | Operator::I32RemU | Operator::I64RemS | Operator::I64RemU => {
                self.bin("%", P_MUL)?
            }
            Operator::I32And | Operator::I64And => self.bin("&", P_BITAND)?,
            Operator::I32Or | Operator::I64Or => self.bin("|", P_BITOR)?,
            Operator::I32Xor | Operator::I64Xor => self.bin("^", P_BITXOR)?,
            Operator::I32Shl | Operator::I64Shl => self.bin("<<", P_SHIFT)?,
            Operator::I32ShrS | Operator::I32ShrU | Operator::I64ShrS | Operator::I64ShrU => {
                self.bin(">>", P_SHIFT)?
            }
            Operator::I32Eq | Operator::I64Eq | Operator::F32Eq | Operator::F64Eq => {
                self.bin("==", P_CMP)?
            }
            Operator::I32Ne | Operator::I64Ne | Operator::F32Ne | Operator::F64Ne => {
                self.bin("!=", P_CMP)?
            }
            Operator::I32LtS
            | Operator::I32LtU
            | Operator::I64LtS
            | Operator::I64LtU
            | Operator::F32Lt
            | Operator::F64Lt => self.bin("<", P_CMP)?,
            Operator::I32GtS
            | Operator::I32GtU
            | Operator::I64GtS
            | Operator::I64GtU
            | Operator::F32Gt
            | Operator::F64Gt => self.bin(">", P_CMP)?,
            Operator::I32LeS
            | Operator::I32LeU
            | Operator::I64LeS
            | Operator::I64LeU
            | Operator::F32Le
            | Operator::F64Le => self.bin("<=", P_CMP)?,
            Operator::I32GeS
            | Operator::I32GeU
            | Operator::I64GeS
            | Operator::I64GeU
            | Operator::F32Ge
            | Operator::F64Ge => self.bin(">=", P_CMP)?,
            Operator::I32Eqz | Operator::I64Eqz => self.unary("eqz")?,

            // ---- integer/float conversions (value-preserving casts) ----
            Operator::I32WrapI64 => self.cast("int")?,
            Operator::I64ExtendI32S | Operator::I64ExtendI32U => self.cast("long")?,
            Operator::I32Extend8S | Operator::I32Extend16S => self.cast("int")?,
            Operator::I64Extend8S | Operator::I64Extend16S | Operator::I64Extend32S => {
                self.cast("long")?
            }
            Operator::I32TruncF32S
            | Operator::I32TruncF32U
            | Operator::I32TruncF64S
            | Operator::I32TruncF64U
            | Operator::I32TruncSatF32S
            | Operator::I32TruncSatF32U
            | Operator::I32TruncSatF64S
            | Operator::I32TruncSatF64U => self.cast("int")?,
            Operator::I64TruncF32S
            | Operator::I64TruncF32U
            | Operator::I64TruncF64S
            | Operator::I64TruncF64U
            | Operator::I64TruncSatF32S
            | Operator::I64TruncSatF32U
            | Operator::I64TruncSatF64S
            | Operator::I64TruncSatF64U => self.cast("long")?,
            Operator::F32ConvertI32S
            | Operator::F32ConvertI32U
            | Operator::F32ConvertI64S
            | Operator::F32ConvertI64U
            | Operator::F32DemoteF64 => self.cast("float")?,
            Operator::F64ConvertI32S
            | Operator::F64ConvertI32U
            | Operator::F64ConvertI64S
            | Operator::F64ConvertI64U
            | Operator::F64PromoteF32 => self.cast("double")?,
            Operator::I32ReinterpretF32 => self.cast("int")?,
            Operator::I64ReinterpretF64 => self.cast("long")?,
            Operator::F32ReinterpretI32 => self.cast("float")?,
            Operator::F64ReinterpretI64 => self.cast("double")?,

            // ---- extra integer ops ----
            Operator::I32Rotl | Operator::I64Rotl => self.binfn("rotl")?,
            Operator::I32Rotr | Operator::I64Rotr => self.binfn("rotr")?,
            Operator::I32Clz | Operator::I64Clz => self.unary("clz")?,
            Operator::I32Ctz | Operator::I64Ctz => self.unary("ctz")?,
            Operator::I32Popcnt | Operator::I64Popcnt => self.unary("popcnt")?,
            Operator::F32Abs | Operator::F64Abs => self.unary("abs")?,
            Operator::F32Neg | Operator::F64Neg => self.unary("neg")?,
            Operator::F32Sqrt | Operator::F64Sqrt => self.unary("sqrt")?,
            Operator::F32Min | Operator::F64Min => self.binfn("min")?,
            Operator::F32Max | Operator::F64Max => self.binfn("max")?,

            // ---- bulk memory ----
            Operator::MemoryCopy { .. } => {
                let n = self.pop()?;
                let src = self.pop()?;
                let dst = self.pop()?;
                self.emit(
                    format!("memory_copy({}, {}, {});", dst.text, src.text, n.text),
                    at,
                );
            }
            Operator::MemoryFill { .. } => {
                let n = self.pop()?;
                let val = self.pop()?;
                let dst = self.pop()?;
                self.emit(
                    format!("memory_fill({}, {}, {});", dst.text, val.text, n.text),
                    at,
                );
            }
            Operator::MemorySize { .. } => self.push("memory_size()".into(), P_ATOM),
            Operator::MemoryGrow { .. } => {
                let n = self.pop()?;
                self.push(format!("memory_grow({})", n.text), P_ATOM);
            }

            // ---- memory ----
            Operator::I32Load { memarg } => self.load("int", memarg.offset)?,
            Operator::I64Load { memarg } => self.load("long", memarg.offset)?,
            Operator::F32Load { memarg } => self.load("float", memarg.offset)?,
            Operator::F64Load { memarg } => self.load("double", memarg.offset)?,
            Operator::I32Load8U { memarg } => self.load("ubyte", memarg.offset)?,
            Operator::I32Load8S { memarg } => self.load("byte", memarg.offset)?,
            Operator::I32Load16U { memarg } => self.load("ushort", memarg.offset)?,
            Operator::I32Load16S { memarg } => self.load("short", memarg.offset)?,
            Operator::I64Load8U { memarg } => self.load("ubyte", memarg.offset)?,
            Operator::I64Load8S { memarg } => self.load("byte", memarg.offset)?,
            Operator::I64Load16U { memarg } => self.load("ushort", memarg.offset)?,
            Operator::I64Load16S { memarg } => self.load("short", memarg.offset)?,
            Operator::I64Load32U { memarg } => self.load("uint", memarg.offset)?,
            Operator::I64Load32S { memarg } => self.load("int", memarg.offset)?,
            Operator::I32Store { memarg } => self.store("int", memarg.offset, at)?,
            Operator::I64Store { memarg } => self.store("long", memarg.offset, at)?,
            Operator::F32Store { memarg } => self.store("float", memarg.offset, at)?,
            Operator::F64Store { memarg } => self.store("double", memarg.offset, at)?,
            Operator::I32Store8 { memarg } | Operator::I64Store8 { memarg } => {
                self.store("byte", memarg.offset, at)?
            }
            Operator::I32Store16 { memarg } | Operator::I64Store16 { memarg } => {
                self.store("short", memarg.offset, at)?
            }
            Operator::I64Store32 { memarg } => self.store("int", memarg.offset, at)?,

            // ---- drop / select ----
            Operator::Drop => {
                let v = self.pop()?;
                // Preserve dropped values that can have observable effects: calls
                // (side effects) and memory loads (can trap). Mirrors
                // wasm-decompile emitting dropped expressions as statements.
                if v.text.contains('(') || v.text.contains('[') {
                    self.emit(format!("{};", v.text), at);
                }
            }
            Operator::Select | Operator::TypedSelect { .. } => {
                let c = self.pop()?;
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(
                    format!("select_if({}, {}, {})", c.text, a.text, b.text),
                    P_ATOM,
                );
            }

            // ---- calls (arity from the callee's declared signature) ----
            Operator::Call { function_index } => self.call_direct(*function_index, at)?,
            Operator::ReturnCall { function_index } => {
                self.call_direct(*function_index, at)?;
                // A tail call returns the callee result directly.
                if let Some(v) = self.stack.pop() {
                    self.emit(format!("return {};", v.text), at);
                }
            }
            Operator::CallIndirect { .. } | Operator::ReturnCallIndirect { .. } => {
                // Indirect targets are never resolved; keep this conservative
                // by degrading rather than guessing operand arity.
                return Err("UNSUPPORTED_INDIRECT: call_indirect not reconstructed".into());
            }
            Operator::RefFunc { function_index } => {
                self.push(self.func_ref_name(*function_index), P_ATOM)
            }
            Operator::RefNull { .. } => self.push("null".into(), P_ATOM),

            // ---- control flow ----
            Operator::Block { blockty } => {
                self.require_empty_block(blockty)?;
                let label = self.new_label();
                self.frames.push(Frame {
                    kind: FrameKind::Block,
                    label,
                    used_as_break: false,
                    lines: Vec::new(),
                    else_lines: None,
                    in_else: false,
                });
            }
            Operator::Loop { blockty } => {
                self.require_empty_block(blockty)?;
                let label = self.new_label();
                self.frames.push(Frame {
                    kind: FrameKind::Loop,
                    label,
                    used_as_break: false,
                    lines: Vec::new(),
                    else_lines: None,
                    in_else: false,
                });
            }
            Operator::If { blockty } => {
                self.require_empty_block(blockty)?;
                let cond = self.pop()?;
                let label = self.new_label();
                let mut frame = Frame {
                    kind: FrameKind::If,
                    label,
                    used_as_break: false,
                    lines: Vec::new(),
                    else_lines: None,
                    in_else: false,
                };
                frame.lines.push(Line {
                    text: format!("__COND__{}", cond.text),
                    offset: at,
                });
                self.frames.push(frame);
            }
            Operator::Else => {
                let frame = self.frames.last_mut().unwrap();
                if !matches!(frame.kind, FrameKind::If) {
                    return Err("UNSUPPORTED_CFG: else without if".into());
                }
                frame.in_else = true;
                frame.else_lines.get_or_insert_with(Vec::new);
            }
            Operator::Br { relative_depth } => {
                let (label, is_loop) = self.branch_target(*relative_depth)?;
                if is_loop {
                    self.emit(format!("continue {label};"), at);
                } else {
                    self.emit(format!("goto {label};"), at);
                }
            }
            Operator::BrIf { relative_depth } => {
                let cond = self.pop()?;
                let (label, is_loop) = self.branch_target(*relative_depth)?;
                let verb = if is_loop { "continue" } else { "goto" };
                self.emit(format!("if ({}) {verb} {label};", cond.text), at);
            }
            Operator::BrTable { targets } => {
                let selector = self.pop()?;
                let default = targets.default();
                let mut arms = Vec::new();
                for (case, target) in targets.targets().enumerate() {
                    let depth = target
                        .map_err(|_| "UNSUPPORTED_BRANCH: br_table target decode".to_string())?;
                    let (label, is_loop) = self.branch_target(depth)?;
                    let verb = if is_loop { "continue" } else { "goto" };
                    arms.push(format!("case {case}: {verb} {label};"));
                }
                let (dlabel, dloop) = self.branch_target(default)?;
                let dverb = if dloop { "continue" } else { "goto" };
                let body = arms.join(" ");
                self.emit(
                    format!(
                        "switch ({}) {{ {body} default: {dverb} {dlabel}; }}",
                        selector.text
                    ),
                    at,
                );
            }
            Operator::Return => {
                if self.func_results > 0 {
                    let v = self.pop()?;
                    self.emit(format!("return {};", v.text), at);
                } else {
                    self.emit("return;".into(), at);
                }
            }
            Operator::Unreachable => self.emit("unreachable;".into(), at),
            Operator::Nop => {}
            Operator::End => return self.end_frame(at),

            // Anything not explicitly handled is uncovered: degrade safely.
            _ => {
                return Err(format!(
                    "UNSUPPORTED_OPCODE: {}",
                    crate::view::op_mnemonic(op)
                ));
            }
        }
        Ok(())
    }

    fn assign_local(&mut self, index: u32, value: &str) -> String {
        let name = self.local_name(index);
        if self.declared.insert(index as usize) {
            let ty = self
                .local_types
                .get(index as usize)
                .map(|t| ctype(t))
                .unwrap_or("int");
            format!("var {name}:{ty} = {value};")
        } else {
            format!("{name} = {value};")
        }
    }

    fn func_ref_name(&self, function_index: u32) -> String {
        match self.module.func_label(function_index) {
            Some(name) => name.to_string(),
            None => format!("f{function_index}"),
        }
    }

    fn call_direct(&mut self, function_index: u32, at: usize) -> std::result::Result<(), String> {
        let (params, results) = self
            .module
            .func_arity(function_index)
            .ok_or_else(|| "UNSUPPORTED_CALL: unknown callee arity".to_string())?;
        if self.stack.len() < params {
            return Err("UNSUPPORTED_STACK: call operand underflow".into());
        }
        let mut args = Vec::with_capacity(params);
        for _ in 0..params {
            args.push(self.pop()?.text);
        }
        args.reverse();
        let name = self.func_ref_name(function_index);
        let call = format!("{name}({})", args.join(", "));
        match results {
            0 => self.emit(format!("{call};"), at),
            1 => self.push(call, P_ATOM),
            _ => {
                // Multi-result calls materialize into temporaries.
                let base = self.temp_counter;
                self.temp_counter += results;
                let names: Vec<String> = (0..results).map(|i| format!("t{}", base + i)).collect();
                self.emit(format!("{} = {call};", names.join(", ")), at);
                for n in names {
                    self.push(n, P_ATOM);
                }
            }
        }
        Ok(())
    }

    fn require_empty_block(&self, blockty: &BlockType) -> std::result::Result<(), String> {
        match blockty {
            BlockType::Empty | BlockType::Type(_) => Ok(()),
            // Block parameter types permute the stack across the boundary; the
            // conservative reconstruction does not model that yet.
            BlockType::FuncType(_) => Err("UNSUPPORTED_CFG: block with parameter signature".into()),
        }
    }

    fn end_frame(&mut self, at: usize) -> std::result::Result<(), String> {
        let frame = self.frames.pop().unwrap();
        if self.frames.is_empty() {
            // Function end: implicit return of a single trailing stack value.
            let mut lines = frame.lines;
            if self.func_results > 0
                && let Some(v) = self.stack.pop()
            {
                lines.push(Line {
                    text: format!("return {};", v.text),
                    offset: at,
                });
            }
            self.frames.push(Frame {
                kind: FrameKind::Block,
                label: "FUNC".into(),
                used_as_break: false,
                lines,
                else_lines: None,
                in_else: false,
            });
            return Ok(());
        }
        let rendered = self.render_frame(frame, at);
        for line in rendered {
            self.frames.last_mut().unwrap().lines.push(line);
        }
        Ok(())
    }

    fn render_frame(&self, frame: Frame, at: usize) -> Vec<Line> {
        let mut out = Vec::new();
        match frame.kind {
            FrameKind::If => {
                let mut cond = String::new();
                let mut then_lines = Vec::new();
                for line in frame.lines {
                    if let Some(c) = line.text.strip_prefix("__COND__") {
                        cond = c.to_string();
                    } else {
                        then_lines.push(line);
                    }
                }
                let head_off = then_lines.first().map(|l| l.offset).unwrap_or(at);
                out.push(Line {
                    text: format!("if ({cond}) {{"),
                    offset: head_off,
                });
                for line in then_lines {
                    out.push(indent(line));
                }
                if let Some(else_lines) = frame.else_lines {
                    out.push(Line {
                        text: "} else {".into(),
                        offset: at,
                    });
                    for line in else_lines {
                        out.push(indent(line));
                    }
                }
                out.push(Line {
                    text: "}".into(),
                    offset: at,
                });
            }
            FrameKind::Loop => {
                out.push(Line {
                    text: format!("loop {} {{", frame.label),
                    offset: frame.lines.first().map(|l| l.offset).unwrap_or(at),
                });
                for line in frame.lines {
                    out.push(indent(line));
                }
                out.push(Line {
                    text: "}".into(),
                    offset: at,
                });
            }
            FrameKind::Block => {
                for line in frame.lines {
                    out.push(line);
                }
                if frame.used_as_break {
                    out.push(Line {
                        text: format!("label {}:", frame.label),
                        offset: at,
                    });
                }
            }
        }
        out
    }

    fn finish(mut self) -> Attempt {
        let frame = self.frames.pop().unwrap();
        let mut code = String::new();
        let mut mapping = Vec::new();
        for (line_no, line) in frame.lines.iter().enumerate() {
            code.push_str(&line.text);
            code.push('\n');
            mapping.push((line_no, line.offset));
        }
        Attempt::Decompiled { code, mapping }
    }
}

fn indent(line: Line) -> Line {
    Line {
        text: format!("  {}", line.text),
        offset: line.offset,
    }
}

/// Byte size of a C-like memory-access element type, for index scaling.
fn type_byte_size(ty: &str) -> u64 {
    match ty {
        "byte" | "ubyte" => 1,
        "short" | "ushort" => 2,
        "int" | "uint" | "float" => 4,
        "long" | "double" => 8,
        _ => 1,
    }
}

/// Detect a scaled dynamic index at the tail of an address expression:
/// `BASE + (IDX << k)` or bare `(IDX << k)`, where `2^k == size`. Returns
/// `(BASE_or_0, IDX)` on success. Operates on the already-rendered expression,
/// so it strips one layer of wrapping parens and matches the trailing shift
/// term only; anything it cannot prove is left untouched.
fn split_scaled_index(base: &str, size: u64) -> Option<(String, String)> {
    let want_shift = size.trailing_zeros() as u64; // 4 -> 2, 8 -> 3
    if 1u64 << want_shift != size {
        return None;
    }
    // Strip one balanced outer paren pair if it wraps the whole expression.
    let expr = strip_outer_parens(base.trim());
    if !expr.ends_with(')') {
        return None;
    }
    let open = find_matching_open(expr)?; // index of '(' that opens the final group
    let inner = &expr[open + 1..expr.len() - 1];
    let idx = inner.strip_suffix(&format!(" << {want_shift}"))?;
    // What precedes the final `(... << k)` group must be empty or `BASE + `.
    let head = expr[..open].trim_end();
    let outer = if head.is_empty() {
        "0".to_string()
    } else {
        let b = head.strip_suffix('+')?.trim();
        if b.is_empty() {
            return None;
        }
        b.to_string()
    };
    // Index must be a simple, side-effect-free term (no call, no nested index).
    if idx.is_empty() || idx.contains('(') || idx.contains('[') {
        return None;
    }
    Some((outer, idx.to_string()))
}

/// If `s` is wrapped in one balanced outer paren pair, return the inside.
fn strip_outer_parens(s: &str) -> &str {
    if !(s.starts_with('(') && s.ends_with(')')) {
        return s;
    }
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    // Balanced close before the end means the outer parens do
                    // not wrap the whole expression.
                    return if i == s.len() - 1 {
                        &s[1..s.len() - 1]
                    } else {
                        s
                    };
                }
            }
            _ => {}
        }
    }
    s
}

/// Index of the '(' that opens the final top-level parenthesized group.
fn find_matching_open(expr: &str) -> Option<usize> {
    if !expr.ends_with(')') {
        return None;
    }
    let bytes = expr.as_bytes();
    let mut depth = 0i32;
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Map a wasm value-type name to the C-like scalar used in output.
fn ctype(ty: &str) -> &'static str {
    match ty {
        "i32" => "int",
        "i64" => "long",
        "f32" => "float",
        "f64" => "double",
        _ => "int",
    }
}

/// Render the full function (signature + body) for a successful attempt.
pub fn render(module: &Module, position: usize, code: &str) -> String {
    let function = module.function_at(position);
    let name = function
        .name
        .clone()
        .unwrap_or_else(|| format!("f{}", function.func_index));
    let args: Vec<String> = function
        .params
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let n = if i < 26 {
                ((b'a' + i as u8) as char).to_string()
            } else {
                format!("v{i}")
            };
            format!("{n}:{}", ctype(t))
        })
        .collect();
    let ret = function
        .results
        .first()
        .map(|t| format!(":{}", ctype(t)))
        .unwrap_or_default();
    let mut out = format!("function {name}({}){ret} {{\n", args.join(", "));
    for line in code.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

/// Build the final decompiled `FunctionView` or signal degradation.
pub fn view(module: &Module, position: usize) -> Result<std::result::Result<FunctionView, String>> {
    match decompile(module, position)? {
        Attempt::Decompiled { code, mapping } => {
            let function = module.function_at(position).clone();
            let rendered = render(module, position, &code);
            // Mapping offsets are body-relative; shift line numbers by the
            // signature line so they index the rendered output.
            let mapping = mapping
                .into_iter()
                .map(|(line, offset)| (line + 1, offset))
                .collect();
            Ok(Ok(FunctionView {
                representation: "clike",
                decompiled: true,
                reason: None,
                code: Some(rendered),
                func_index: function.func_index,
                type_index: function.type_index,
                name: function.name,
                params: function.params,
                results: function.results,
                locals: Vec::new(),
                body_file_offset: function.body_file_offset,
                body_size: function.body_size,
                bytecode_hex: module.body_hex(position),
                instructions: Vec::new(),
                mapping,
            }))
        }
        Attempt::Degraded { reason } => Ok(Err(reason)),
    }
}
