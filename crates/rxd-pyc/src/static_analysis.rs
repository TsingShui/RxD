//! Static PYC metadata and marshal evidence; no instruction scan.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    rc::Rc,
};

use pycdc::{
    code::CodeObject, evidence::MarshalEvidence, marshal::MarshalReader, object::PyObject,
    version::PythonVersion,
};
use rxd_core::{Error, Result, perf::Perf};

#[derive(Clone, Copy)]
pub struct Limits {
    pub input_bytes: usize,
    pub entities: usize,
    pub metadata_bytes: usize,
    pub marshal: pycdc::marshal::Limits,
    pub work: Option<usize>,
}
impl Limits {
    pub fn perf_limits(self) -> BTreeMap<&'static str, usize> {
        let mut limits = BTreeMap::from([
            ("input_bytes", self.input_bytes),
            ("entities", self.entities),
            ("metadata_bytes", self.metadata_bytes),
            ("marshal_objects", self.marshal.objects),
            ("marshal_expanded_objects", self.marshal.expanded_objects),
            ("marshal_copied_bytes", self.marshal.copied_bytes),
        ]);
        if let Some(work) = self.work {
            limits.insert("work", work);
        }
        limits
    }
}

pub struct CodeEntry {
    pub code: Rc<CodeObject>,
    pub definition: usize,
}

pub struct Analysis {
    pub bytes: Vec<u8>,
    pub source_id: String,
    pub namespace: Option<String>,
    pub version: PythonVersion,
    pub evidence: MarshalEvidence,
    pub codes: Vec<CodeEntry>,
    pub limits: Limits,
    code_indexes: HashMap<usize, usize>,
}

impl Analysis {
    pub fn load(
        path: &Path,
        version: Option<(u8, u8)>,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Self> {
        perf.resource_limits = Some(limits.perf_limits());
        let bytes = perf.measure("read", || rxd_core::read_bytes(path, limits.input_bytes))?;
        perf.counters.input_bytes = Some(bytes.len());
        let source_id = perf.measure("hash", || rxd_core::identity::input_id(&bytes));
        perf.source_id = Some(source_id.clone());
        let (version, header) =
            pycdc::loader::input_format(&bytes, version).map_err(super::input_error)?;
        Self::from_bytes(
            bytes,
            version,
            header.map_or(0, |header| header.header_size),
            None,
            source_id,
            limits,
            perf,
        )
    }

    pub(crate) fn from_bytes(
        bytes: Vec<u8>,
        version: PythonVersion,
        base: usize,
        namespace: Option<String>,
        source_id: String,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Self> {
        let mut parse_counts = None;
        let parsed = perf.measure("parse", || -> Result<_> {
            let mut reader = MarshalReader::with_evidence(
                &bytes[base..],
                version,
                base,
                limits.entities,
                limits.work,
            )
            .with_limits(limits.marshal);
            let result = reader.load();
            parse_counts = reader.evidence_counts();
            let object = result.map_err(super::input_error)?;
            if !reader.at_eof() {
                return Err(Error::new("INVALID_INPUT", "trailing marshal data"));
            }
            if !matches!(&*object, PyObject::Code(_)) {
                return Err(Error::new(
                    "INVALID_INPUT",
                    "top-level object is not a code object",
                ));
            }
            Ok(reader.into_evidence().unwrap())
        });
        if let Some((entities, work)) = parse_counts {
            perf.counters.entities = Some(entities);
            perf.counters.work = Some(work);
        }
        let evidence = parsed?;
        let mut analysis = Self {
            bytes,
            source_id,
            namespace,
            version,
            evidence,
            codes: Vec::new(),
            limits,
            code_indexes: HashMap::new(),
        };
        let indexed = perf.measure("visit", || analysis.add_code(0).map(|_| ()));
        perf.counters.entities = Some(analysis.evidence.entities);
        perf.counters.work = Some(analysis.evidence.work);
        indexed?;
        Ok(analysis)
    }

    pub fn charge_work(&mut self, amount: usize) -> Result<()> {
        if self
            .limits
            .work
            .is_some_and(|limit| amount > limit.saturating_sub(self.evidence.work))
        {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "static traversal work budget exceeded",
            ));
        }
        self.evidence.work += amount;
        Ok(())
    }
    pub fn local_id<'a>(&self, id: &'a str) -> Result<&'a str> {
        match self.namespace.as_deref() {
            None => Ok(id),
            Some(namespace) if id == namespace => Ok("code:0"),
            Some(namespace) => id
                .strip_prefix(namespace)
                .and_then(|id| id.strip_prefix("::"))
                .ok_or_else(|| {
                    Error::new("INVALID_QUERY", "coordinate belongs to another PYZ module")
                }),
        }
    }

    pub fn parse_code_id(&mut self, id: &str) -> Result<usize> {
        self.charge_work(id.len())?;
        let index = crate::input::code_index(self.local_id(id)?)?;
        if index >= self.codes.len() {
            return Err(Error::new(
                "NOT_FOUND",
                format!("no code coordinate {id:?} in this input"),
            ));
        }
        Ok(index)
    }

    /// Select code rows by the closed listing surface: an optional case-insensitive
    /// substring, an optional exact name, and an optional exact index.
    pub fn select_codes(
        &mut self,
        filter: Option<&rxd_core::filter::Contains>,
        name: Option<&str>,
        index: Option<usize>,
    ) -> Result<Vec<usize>> {
        if index.is_some_and(|index| index >= self.codes.len()) {
            return Err(Error::failure(
                rxd_core::Failure::NotFound,
                "code index does not exist",
            ));
        }

        let mut selected = Vec::new();
        let scope = index.map_or(0..self.codes.len(), |i| i..i + 1);
        for i in scope {
            self.charge_work(1)?;
            let name_matches = match name {
                None => true,
                Some(name) => {
                    let candidate = self.text_field(i, "name").map(str::to_owned);
                    if candidate.as_deref().map(str::len) == Some(name.len()) {
                        self.charge_work(name.len())?;
                        candidate.as_deref() == Some(name)
                    } else {
                        false
                    }
                }
            };
            let filter_matches = match filter.as_ref() {
                None => true,
                Some(filter) => {
                    let candidate = self
                        .text_field(i, "qualname")
                        .or_else(|| self.text_field(i, "name"))
                        .unwrap_or_default()
                        .to_owned();
                    self.charge_work(
                        candidate
                            .len()
                            .saturating_add(self.namespace.as_ref().map_or(0, String::len)),
                    )?;
                    filter.matches(&candidate)
                        || self
                            .namespace
                            .as_deref()
                            .is_some_and(|name| filter.matches(name))
                }
            };
            if name_matches && filter_matches {
                selected.push(i);
            }
        }
        Ok(selected)
    }
    pub fn qualify_id(&self, local: &str) -> String {
        match self.namespace.as_deref() {
            Some(namespace) if local == "code:0" => namespace.to_owned(),
            Some(namespace) => format!("{namespace}::{local}"),
            None => local.to_owned(),
        }
    }
    pub fn code_id(&self, index: usize) -> String {
        self.qualify_id(&format!("code:{index}"))
    }
    pub fn field_occurrence(&self, index: usize, name: &str) -> Option<usize> {
        self.evidence.occurrences[self.codes[index].definition]
            .fields
            .get(name)?
            .occurrence
    }
    pub fn field_definition(&self, index: usize, name: &str) -> Option<usize> {
        Some(self.evidence.occurrences[self.field_occurrence(index, name)?].definition)
    }
    pub fn payload(&self, definition: usize) -> Option<&[u8]> {
        let span = self.evidence.occurrences[definition].payload?;
        self.bytes.get(span.start..span.end)
    }
    /// Strict UTF-8 view; surrogatepass bytes remain in the fixed input evidence.
    pub fn text_field(&self, index: usize, name: &str) -> Option<&str> {
        std::str::from_utf8(self.payload(self.field_definition(index, name)?)?).ok()
    }
    pub fn body_status(&self, index: usize) -> &'static str {
        let Some(definition) = self.field_definition(index, "code") else {
            return "unavailable";
        };
        match self.evidence.occurrences[definition].object.as_deref() {
            Some(PyObject::Bytes(bytes)) if bytes.is_empty() => "empty",
            Some(PyObject::Bytes(_)) => "present",
            _ => "unavailable",
        }
    }
    fn add_code(&mut self, occurrence: usize) -> Result<usize> {
        self.charge_work(1)?;
        let definition = self.evidence.occurrences[occurrence].definition;
        if let Some(&index) = self.code_indexes.get(&definition) {
            return Ok(index);
        }
        let Some(PyObject::Code(code)) = self.evidence.occurrences[definition].object.as_deref()
        else {
            return Err(Error::new(
                "INVALID_INPUT",
                "code evidence does not identify a code object",
            ));
        };
        let index = self.codes.len();
        self.codes.push(CodeEntry {
            code: code.clone(),
            definition,
        });
        self.code_indexes.insert(definition, index);
        let consts_occurrence = self.field_occurrence(index, "consts").unwrap();
        let consts_row = &self.evidence.occurrences[consts_occurrence];
        let consts = consts_row.definition;
        self.charge_work(self.evidence.occurrences[consts].children.len())?;
        let children = self.evidence.occurrences[consts].children.clone();
        let mut visited = HashSet::new();
        for child in children {
            self.walk_constant(child, &mut visited)?;
        }
        Ok(index)
    }
    fn walk_constant(&mut self, occurrence: usize, visited: &mut HashSet<usize>) -> Result<()> {
        self.charge_work(1)?;
        let row = &self.evidence.occurrences[occurrence];
        let definition = row.definition;
        let object = self.evidence.occurrences[definition]
            .object
            .as_deref()
            .unwrap();
        if matches!(object, PyObject::Code(_)) {
            self.add_code(occurrence)?;
            return Ok(());
        }
        if !matches!(
            object,
            PyObject::Tuple(_)
                | PyObject::List(_)
                | PyObject::Dict(_)
                | PyObject::Set(_)
                | PyObject::FrozenSet(_)
                | PyObject::Slice(..)
        ) || !visited.insert(definition)
        {
            return Ok(());
        }
        self.charge_work(self.evidence.occurrences[definition].children.len())?;
        let children = self.evidence.occurrences[definition].children.clone();
        for child in children {
            self.walk_constant(child, visited)?;
        }
        Ok(())
    }
}
