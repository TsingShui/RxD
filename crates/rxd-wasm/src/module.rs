use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, io::Read, ops::Range, path::Path};

use anyhow::{Context, Result, ensure};
use wasmparser::{
    BinaryReader, CompositeInnerType, Name, NameSectionReader, Operator, Parser, Payload, ValType,
};

use crate::model::*;

/// A validated, bounded WebAssembly module held as one fixed binary buffer.
pub struct Module {
    pub source_id: String,
    pub file_bytes: usize,
    bytes: Vec<u8>,
    limits: Limits,

    pub func_types: Vec<FuncType>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub functions: Vec<Function>,
    pub tables: Vec<Table>,
    pub memories: Vec<Memory>,
    pub globals: Vec<Global>,
    pub data_segments: Vec<DataSegment>,
    pub element_segments: Vec<ElementSegment>,

    /// Number of imported functions; defined functions start at this index.
    imported_funcs: u32,
    /// For each defined function (by position), the raw type index.
    defined_type_index: Vec<u32>,
    /// For each defined function (by position), the code-body byte range in the file.
    defined_body_range: Vec<Range<usize>>,
    /// Raw bytes of each data segment, parallel to `data_segments`.
    data_bytes: Vec<Vec<u8>>,
}

impl Module {
    pub fn read(path: &Path, limits: Limits) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = options
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file(),
            "INVALID_INPUT: input must be a regular file"
        );
        ensure!(
            meta.len() <= limits.input_bytes as u64,
            "RESOURCE_LIMIT: input bytes"
        );
        let mut bytes = Vec::new();
        file.by_ref()
            .take((limits.input_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= limits.input_bytes && bytes.len() <= u32::MAX as usize,
            "RESOURCE_LIMIT: input bytes"
        );
        Self::parse_bytes(bytes, limits, Some(path))
    }

    pub fn from_bytes(bytes: Vec<u8>, limits: Limits) -> Result<Self> {
        Self::parse_bytes(bytes, limits, None)
    }

    fn parse_bytes(bytes: Vec<u8>, limits: Limits, path: Option<&Path>) -> Result<Self> {
        ensure!(
            bytes.len() >= 8 && &bytes[0..4] == b"\0asm",
            "INVALID_INPUT: not a WebAssembly binary (bad magic)"
        );
        let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        ensure!(version == 1, "INVALID_INPUT: unsupported wasm version");

        let source_id = path.map_or_else(
            || format!("{:x}", Sha256::digest(&bytes)),
            |path| rxd_core::identity::content_id(path, &bytes),
        );
        let file_bytes = bytes.len();

        let mut module = Module {
            source_id,
            file_bytes,
            bytes: Vec::new(),
            limits,
            func_types: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            functions: Vec::new(),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
            data_segments: Vec::new(),
            element_segments: Vec::new(),
            imported_funcs: 0,
            defined_type_index: Vec::new(),
            defined_body_range: Vec::new(),
            data_bytes: Vec::new(),
        };
        module.decode(&bytes)?;
        module.bytes = bytes;
        Ok(module)
    }

    fn charge_entities(&self, running: &mut usize, add: usize) -> Result<()> {
        *running = running.saturating_add(add);
        ensure!(*running <= self.limits.entities, "RESOURCE_LIMIT: entities");
        Ok(())
    }

    fn decode(&mut self, bytes: &[u8]) -> Result<()> {
        // Raw type indices for the function section, resolved to signatures later.
        let mut func_section: Vec<u32> = Vec::new();
        // Name-section labels, applied after structure decode.
        let mut func_names: std::collections::BTreeMap<u32, String> =
            std::collections::BTreeMap::new();
        let mut global_names: std::collections::BTreeMap<u32, String> =
            std::collections::BTreeMap::new();
        let mut entities = 0usize;

        for payload in Parser::new(0).parse_all(bytes) {
            let payload = payload.context("INVALID_INPUT: malformed section")?;
            match payload {
                Payload::TypeSection(reader) => {
                    for group in reader {
                        let group = group.context("INVALID_INPUT: type section")?;
                        for ty in group.into_types() {
                            self.charge_entities(&mut entities, 1)?;
                            let type_index = self.func_types.len() as u32;
                            if let CompositeInnerType::Func(ft) = ty.composite_type.inner {
                                self.func_types.push(FuncType {
                                    type_index,
                                    params: ft.params().iter().map(valtype_name).collect(),
                                    results: ft.results().iter().map(valtype_name).collect(),
                                });
                            } else {
                                // Non-function composite types still occupy a type index.
                                self.func_types.push(FuncType {
                                    type_index,
                                    params: Vec::new(),
                                    results: Vec::new(),
                                });
                            }
                        }
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader {
                        let import = import.context("INVALID_INPUT: import section")?;
                        self.charge_entities(&mut entities, 1)?;
                        let (kind, func_index, type_index) = match import.ty {
                            wasmparser::TypeRef::Func(t) => {
                                let idx = self.imported_funcs;
                                self.imported_funcs += 1;
                                ("func", Some(idx), Some(t))
                            }
                            wasmparser::TypeRef::Table(_) => ("table", None, None),
                            wasmparser::TypeRef::Memory(_) => ("memory", None, None),
                            wasmparser::TypeRef::Global(_) => ("global", None, None),
                            wasmparser::TypeRef::Tag(_) => ("tag", None, None),
                        };
                        self.imports.push(Import {
                            module: import.module.to_string(),
                            name: import.name.to_string(),
                            kind,
                            func_index,
                            type_index,
                        });
                    }
                }
                Payload::FunctionSection(reader) => {
                    for item in reader {
                        let type_index = item.context("INVALID_INPUT: function section")?;
                        self.charge_entities(&mut entities, 1)?;
                        func_section.push(type_index);
                    }
                }
                Payload::TableSection(reader) => {
                    for (i, table) in reader.into_iter().enumerate() {
                        let table = table.context("INVALID_INPUT: table section")?;
                        self.charge_entities(&mut entities, 1)?;
                        self.tables.push(Table {
                            index: i as u32,
                            element_type: valtype_name(&ValType::Ref(table.ty.element_type)),
                            initial: table.ty.initial,
                            maximum: table.ty.maximum,
                        });
                    }
                }
                Payload::MemorySection(reader) => {
                    for (i, memory) in reader.into_iter().enumerate() {
                        let memory = memory.context("INVALID_INPUT: memory section")?;
                        self.charge_entities(&mut entities, 1)?;
                        self.memories.push(Memory {
                            index: i as u32,
                            initial: memory.initial,
                            maximum: memory.maximum,
                            memory64: memory.memory64,
                            shared: memory.shared,
                        });
                    }
                }
                Payload::GlobalSection(reader) => {
                    for (i, global) in reader.into_iter().enumerate() {
                        let global = global.context("INVALID_INPUT: global section")?;
                        self.charge_entities(&mut entities, 1)?;
                        self.globals.push(Global {
                            index: i as u32,
                            value_type: valtype_name(&global.ty.content_type),
                            mutable: global.ty.mutable,
                            name: None,
                        });
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.context("INVALID_INPUT: export section")?;
                        self.charge_entities(&mut entities, 1)?;
                        let kind = match export.kind {
                            wasmparser::ExternalKind::Func => "func",
                            wasmparser::ExternalKind::Table => "table",
                            wasmparser::ExternalKind::Memory => "memory",
                            wasmparser::ExternalKind::Global => "global",
                            wasmparser::ExternalKind::Tag => "tag",
                        };
                        self.exports.push(Export {
                            name: export.name.to_string(),
                            kind,
                            index: export.index,
                        });
                    }
                }
                Payload::ElementSection(reader) => {
                    for (i, element) in reader.into_iter().enumerate() {
                        let element = element.context("INVALID_INPUT: element section")?;
                        self.charge_entities(&mut entities, 1)?;
                        let (kind, count) = match &element.items {
                            wasmparser::ElementItems::Functions(f) => {
                                ("funcrefs", f.count() as usize)
                            }
                            wasmparser::ElementItems::Expressions(_, e) => {
                                ("expressions", e.count() as usize)
                            }
                        };
                        let kind = match element.kind {
                            wasmparser::ElementKind::Passive => kind,
                            wasmparser::ElementKind::Active { .. } => "active",
                            wasmparser::ElementKind::Declared => "declared",
                        };
                        self.element_segments.push(ElementSegment {
                            index: i as u32,
                            kind,
                            count,
                        });
                    }
                }
                Payload::DataSection(reader) => {
                    for (i, data) in reader.into_iter().enumerate() {
                        let data = data.context("INVALID_INPUT: data section")?;
                        self.charge_entities(&mut entities, 1)?;
                        let (kind, memory_index, offset_expr) = match data.kind {
                            wasmparser::DataKind::Passive => ("passive", None, None),
                            wasmparser::DataKind::Active {
                                memory_index,
                                offset_expr,
                            } => (
                                "active",
                                Some(memory_index),
                                Some(const_expr_text(&offset_expr)),
                            ),
                        };
                        self.data_segments.push(DataSegment {
                            index: i as u32,
                            kind,
                            memory_index,
                            offset_expr,
                            byte_length: data.data.len(),
                        });
                        self.data_bytes.push(data.data.to_vec());
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let range = body.range();
                    self.defined_body_range.push(range);
                }
                Payload::CustomSection(reader) if reader.name() == "name" => {
                    let br = BinaryReader::new(reader.data(), reader.data_offset());
                    let names = NameSectionReader::new(br);
                    for sub in names {
                        match sub.context("INVALID_INPUT: name section")? {
                            Name::Function(map) => {
                                for naming in map {
                                    let naming = naming.context("INVALID_INPUT: name section")?;
                                    func_names.insert(naming.index, naming.name.to_string());
                                }
                            }
                            Name::Global(map) => {
                                for naming in map {
                                    let naming = naming.context("INVALID_INPUT: name section")?;
                                    global_names.insert(naming.index, naming.name.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // Resolve defined functions: positions in func_section align with code bodies.
        ensure!(
            func_section.len() == self.defined_body_range.len(),
            "INVALID_INPUT: function/code section count mismatch"
        );
        self.defined_type_index = func_section.clone();
        for (position, type_index) in func_section.into_iter().enumerate() {
            let func_index = self.imported_funcs + position as u32;
            let signature = self
                .func_types
                .get(type_index as usize)
                .context("INVALID_INPUT: function type index out of range")?;
            let range = self.defined_body_range[position].clone();
            self.functions.push(Function {
                id: self.func_id(func_index),
                func_index,
                type_index,
                name: func_names.get(&func_index).cloned(),
                params: signature.params.clone(),
                results: signature.results.clone(),
                body_file_offset: range.start,
                body_size: range.end - range.start,
            });
        }
        for global in &mut self.globals {
            global.name = global_names.get(&global.index).cloned();
        }
        Ok(())
    }

    pub fn func_id(&self, func_index: u32) -> String {
        format!("rxd1:wasm:{}:func:{}", self.source_id, func_index)
    }

    /// Signature (param count, result count) for any function index, imported
    /// or defined, resolved through its declared type. Used to drive call
    /// argument arity during reassembly.
    pub fn func_arity(&self, func_index: u32) -> Option<(usize, usize)> {
        let type_index = if func_index < self.imported_funcs {
            self.imports
                .iter()
                .filter(|i| i.kind == "func")
                .find(|i| i.func_index == Some(func_index))
                .and_then(|i| i.type_index)?
        } else {
            let position = (func_index - self.imported_funcs) as usize;
            *self.defined_type_index.get(position)?
        };
        let ty = self.func_types.get(type_index as usize)?;
        Some((ty.params.len(), ty.results.len()))
    }

    /// A function's name-section label, if any (imported or defined).
    pub fn func_label(&self, func_index: u32) -> Option<&str> {
        self.functions
            .iter()
            .find(|f| f.func_index == func_index)
            .and_then(|f| f.name.as_deref())
    }

    pub fn total_functions(&self) -> u32 {
        self.imported_funcs + self.functions.len() as u32
    }

    pub fn imported_func_count(&self) -> u32 {
        self.imported_funcs
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Resolve the `--at` coordinate a `functions` row prints to a defined
    /// function position.
    ///
    /// A row prints the copy-only `func:<index>`; the content identity lives in
    /// the stream header, so a short coordinate is expanded against this
    /// module's own hash here instead of making a caller reassemble it. A full
    /// `rxd1:wasm:<sha256>:func:<index>` id keeps its own hash, so one naming a
    /// different module fails as `SOURCE_MISMATCH` rather than being rewritten.
    /// The function index a `func:N` coordinate names.
    ///
    /// The index space is what call operands carry; a body position is not, so a query that
    /// scans call sites needs this and not the position. An imported function has no body but
    /// does have callers, so only the index space bounds it.
    pub fn function_index_at(&self, at: &str) -> Result<u32> {
        let func_index = self.coordinate_function(at)?;
        ensure!(
            func_index < self.total_functions(),
            rxd_core::Error::failure(
                rxd_core::Failure::NotFound,
                format!("no function func:{func_index} in this module"),
            ),
        );
        Ok(func_index)
    }

    /// The position of a `func:N` coordinate's body in the code section.
    pub fn defined_position_at(&self, at: &str) -> Result<usize> {
        self.defined_position(self.coordinate_function(at)?)
    }

    fn coordinate_function(&self, at: &str) -> Result<u32> {
        let expanded;
        let id = if at.starts_with("rxd1:") {
            at
        } else {
            expanded = format!("rxd1:wasm:{}:{at}", self.source_id);
            expanded.as_str()
        };
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() != 5
            || parts[0] != "rxd1"
            || parts[1] != "wasm"
            || parts[2].len() != 64
            || !parts[2]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(anyhow::anyhow!(
                "INVALID_QUERY: malformed or unsupported WASM coordinate"
            ));
        }
        if parts[2] != self.source_id {
            return Err(anyhow::anyhow!(
                "SOURCE_MISMATCH: coordinate belongs to different source content"
            ));
        }
        ensure!(
            parts[3] == "func",
            "INVALID_QUERY: --at requires a function coordinate"
        );
        parts[4]
            .parse()
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: function index"))
    }

    pub fn defined_position(&self, func_index: u32) -> Result<usize> {
        ensure!(
            func_index < self.total_functions(),
            rxd_core::Error::failure(
                rxd_core::Failure::NotFound,
                format!("no function func:{func_index} in this module"),
            ),
        );
        ensure!(
            func_index >= self.imported_funcs,
            "INVALID_QUERY: imported functions have no code body"
        );
        Ok((func_index - self.imported_funcs) as usize)
    }

    pub fn function_at(&self, position: usize) -> &Function {
        &self.functions[position]
    }

    /// Decode one defined function body into (locals, instruction list).
    /// Returns operators with their byte offset within the code body.
    pub fn decode_body(&self, position: usize) -> Result<DecodedBody> {
        let range = self.defined_body_range[position].clone();
        let body = wasmparser::FunctionBody::new(BinaryReader::new(
            &self.bytes[range.start..range.end],
            range.start,
        ));
        let mut locals = Vec::new();
        for local in body.get_locals_reader().context("INVALID_INPUT: locals")? {
            let (count, ty) = local.context("INVALID_INPUT: locals")?;
            locals.push((count, valtype_name(&ty)));
        }
        let base = range.start;
        let mut reader = body
            .get_operators_reader()
            .context("INVALID_INPUT: operators")?;
        let mut ops = Vec::new();
        let mut budget = 0usize;
        while !reader.eof() {
            let (op, pos) = reader
                .read_with_offset()
                .context("INVALID_INPUT: operator decode")?;
            budget += 1;
            ensure!(
                budget <= self.limits.instructions,
                "RESOURCE_LIMIT: per-function instructions"
            );
            ops.push(DecodedOp {
                code_offset: pos - base,
                op: summarize_op(&op),
            });
        }
        Ok((locals, ops))
    }

    /// Raw body byte range and the full module buffer slice for `position`.
    pub fn body_slice(&self, position: usize) -> (std::ops::Range<usize>, &[u8]) {
        let range = self.defined_body_range[position].clone();
        (range.clone(), &self.bytes[range.start..range.end])
    }

    pub fn body_hex(&self, position: usize) -> String {
        let range = self.defined_body_range[position].clone();
        let mut s = String::with_capacity((range.end - range.start) * 2);
        for b in &self.bytes[range.start..range.end] {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub fn data_segment_bytes(&self, index: usize) -> &[u8] {
        &self.data_bytes[index]
    }
}

/// A decoded local-variable declaration: `(count, value-type name)`.
pub type LocalDecl = (u32, String);

/// Decoded single-function body: local declarations and the operator list.
pub type DecodedBody = (Vec<LocalDecl>, Vec<DecodedOp>);

/// A decoded operator reduced to what the structural/refs layer needs.
#[derive(Clone, Debug)]
pub struct DecodedOp {
    pub code_offset: usize,
    pub op: OpSummary,
}

/// Explicit references and control markers extracted from one operator.
#[derive(Clone, Debug)]
pub enum OpSummary {
    /// `call` / `return_call` to a function index.
    CallDirect {
        opcode: &'static str,
        target: u32,
    },
    /// `ref.func` of a function index.
    RefFunc {
        target: u32,
    },
    /// `call_indirect` — indirect, type/table recorded, never a direct target.
    CallIndirect {
        type_index: u32,
        table_index: u32,
    },
    Other,
}

fn summarize_op(op: &Operator) -> OpSummary {
    match op {
        Operator::Call { function_index } => OpSummary::CallDirect {
            opcode: "call",
            target: *function_index,
        },
        Operator::ReturnCall { function_index } => OpSummary::CallDirect {
            opcode: "return_call",
            target: *function_index,
        },
        Operator::RefFunc { function_index } => OpSummary::RefFunc {
            target: *function_index,
        },
        Operator::CallIndirect {
            type_index,
            table_index,
        } => OpSummary::CallIndirect {
            type_index: *type_index,
            table_index: *table_index,
        },
        Operator::ReturnCallIndirect {
            type_index,
            table_index,
        } => OpSummary::CallIndirect {
            type_index: *type_index,
            table_index: *table_index,
        },
        _ => OpSummary::Other,
    }
}

pub fn valtype_display(ty: &ValType) -> String {
    valtype_name(ty)
}

fn valtype_name(ty: &ValType) -> String {
    match ty {
        ValType::I32 => "i32".into(),
        ValType::I64 => "i64".into(),
        ValType::F32 => "f32".into(),
        ValType::F64 => "f64".into(),
        ValType::V128 => "v128".into(),
        ValType::Ref(r) => {
            if r.is_func_ref() {
                "funcref".into()
            } else if r.is_extern_ref() {
                "externref".into()
            } else {
                "ref".into()
            }
        }
    }
}

fn const_expr_text(expr: &wasmparser::ConstExpr) -> String {
    let mut reader = expr.get_operators_reader();
    let mut parts = Vec::new();
    while let Ok(op) = reader.read() {
        match op {
            Operator::I32Const { value } => parts.push(format!("i32.const {value}")),
            Operator::I64Const { value } => parts.push(format!("i64.const {value}")),
            Operator::GlobalGet { global_index } => {
                parts.push(format!("global.get {global_index}"))
            }
            Operator::End => break,
            other => parts.push(format!("{other:?}")),
        }
    }
    parts.join("; ")
}
