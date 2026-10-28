//! Whole-module recovery with function-local bytecode fallbacks.
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
};

use crate::{
    ast::Stmt,
    bytecode::{decode_evidence, Instruction, RawInstruction},
    code::CodeObject,
    decompiler::Decompiled,
    object::PyObject,
    opcode::table_for,
    version::PythonVersion,
    Result,
};

pub(crate) struct Fallback {
    pub reason: String,
    pub logical: Vec<Instruction>,
    pub raw: Vec<RawInstruction>,
    pub children: Vec<(usize, Vec<Stmt>)>,
}

pub struct Module<'a> {
    codes: Vec<&'a CodeObject>,
    indexes: HashMap<usize, usize>,
    ends: Vec<usize>,
    fallbacks: RefCell<BTreeMap<usize, Fallback>>,
    namespace: Option<String>,
}

impl<'a> Module<'a> {
    pub fn new(code: &'a CodeObject) -> Self {
        let mut module = Self {
            codes: Vec::new(),
            indexes: HashMap::new(),
            ends: Vec::new(),
            fallbacks: RefCell::new(BTreeMap::new()),
            namespace: None,
        };
        module.add_code(code);
        module
    }

    fn add_code(&mut self, code: &'a CodeObject) {
        let _frame = crate::budget::enter();
        let key = code as *const CodeObject as usize;
        if self.indexes.contains_key(&key) {
            return;
        }
        let index = self.codes.len();
        self.indexes.insert(key, index);
        self.codes.push(code);
        self.ends.push(index + 1);
        let mut visited = HashSet::new();
        for object in &code.consts {
            self.walk(object, &mut visited);
        }
        self.ends[index] = self.codes.len();
    }

    fn walk(&mut self, object: &'a PyObject, visited: &mut HashSet<usize>) {
        let _frame = crate::budget::enter();
        if let PyObject::Code(code) = object {
            self.add_code(code);
            return;
        }
        if !matches!(
            object,
            PyObject::Tuple(_)
                | PyObject::List(_)
                | PyObject::Set(_)
                | PyObject::FrozenSet(_)
                | PyObject::Dict(_)
                | PyObject::Slice(..)
        ) || !visited.insert(object as *const PyObject as usize)
        {
            return;
        }
        match object {
            PyObject::Tuple(items)
            | PyObject::List(items)
            | PyObject::Set(items)
            | PyObject::FrozenSet(items) => {
                for item in items {
                    self.walk(item, visited);
                }
            }
            PyObject::Dict(items) => {
                for (key, value) in items {
                    self.walk(key, visited);
                    self.walk(value, visited);
                }
            }
            PyObject::Slice(start, stop, step) => {
                for item in [start, stop, step] {
                    self.walk(item, visited);
                }
            }
            _ => {}
        }
    }

    pub fn named(mut self, namespace: Option<String>) -> Self {
        self.namespace = namespace;
        self
    }

    pub fn decompile(&self, version: PythonVersion) -> Result<Decompiled> {
        self.decompile_at(0, version)
    }

    pub fn decompile_at(&self, index: usize, version: PythonVersion) -> Result<Decompiled> {
        let code = self
            .codes
            .get(index)
            .ok_or(crate::PycError::CodeNotFound(index))?;
        crate::decompiler::decompile_recovering(code, version, &[], self)
    }

    pub fn coordinate(&self, index: usize) -> String {
        match self.namespace.as_deref() {
            Some(namespace) if index == 0 => crate::budget_string!(namespace),
            Some(namespace) => crate::budget_format!("{namespace}::code:{index}"),
            None => crate::budget_format!("code:{index}"),
        }
    }

    pub fn generate(
        &self,
        body: &[Stmt],
        version: PythonVersion,
        limit: Option<usize>,
    ) -> Result<String> {
        self.generate_at(body, version, limit, 0)
    }

    pub fn generate_at(
        &self,
        body: &[Stmt],
        version: PythonVersion,
        limit: Option<usize>,
        index: usize,
    ) -> Result<String> {
        if index >= self.codes.len() {
            return Err(crate::PycError::CodeNotFound(index));
        }
        crate::codegen::generate_recovering(body, version, limit, self, index)
    }

    pub fn complete(&self) -> bool {
        self.complete_at(0)
    }

    pub fn complete_at(&self, index: usize) -> bool {
        self.ends
            .get(index)
            .is_some_and(|end| self.fallbacks.borrow().range(index..*end).next().is_none())
    }

    pub(crate) fn index(&self, code: &CodeObject) -> usize {
        self.indexes[&(code as *const CodeObject as usize)]
    }

    pub(crate) fn code(&self, index: usize) -> &CodeObject {
        self.codes[index]
    }

    pub(crate) fn cached_fallback(&self, code: &CodeObject) -> Option<Decompiled> {
        let index = self.index(code);
        self.fallbacks
            .borrow()
            .contains_key(&index)
            .then(|| Self::fallback_body(index))
    }

    fn fallback_body(index: usize) -> Decompiled {
        Decompiled {
            body: vec![Stmt::BytecodeFallback(index)],
            clean: false,
            tail_pair: false,
        }
    }

    pub(crate) fn with_fallback<T>(&self, index: usize, action: impl FnOnce(&Fallback) -> T) -> T {
        action(&self.fallbacks.borrow()[&index])
    }

    pub(crate) fn fallback(
        &self,
        code: &CodeObject,
        version: PythonVersion,
        reason: String,
    ) -> Result<Decompiled> {
        let index = self.index(code);
        if let Some(cached) = self.cached_fallback(code) {
            return Ok(cached);
        }
        let (logical, raw) = decode_evidence(code, table_for(version)?, version)?;
        let mut children = Vec::new();
        let mut child = index + 1;
        while child < self.ends[index] {
            let body =
                crate::decompiler::decompile_recovering(self.codes[child], version, &[], self)?
                    .body;
            children.push((child, body));
            child = self.ends[child];
        }
        self.fallbacks
            .borrow_mut()
            .entry(index)
            .or_insert(Fallback {
                reason,
                logical,
                raw,
                children,
            });
        Ok(Self::fallback_body(index))
    }
}
