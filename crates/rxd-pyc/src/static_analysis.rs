//! Bounded static PYC metadata from the existing marshal parser. No AST or instruction scan.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    rc::Rc,
};

use pycdc::{
    code::CodeObject,
    evidence::{MarshalEvidence, Span},
    marshal::MarshalReader,
    object::PyObject,
    pyc::PycModule,
    version::{PythonVersion, version_from_magic},
};
use rxd_core::{Error, Result, perf::Perf};

#[derive(Clone, Copy)]
pub struct Limits {
    pub input_bytes: usize,
    pub entities: usize,
    pub work: usize,
    pub records: usize,
    pub output_bytes: usize,
}
impl Limits {
    pub fn perf_limits(self) -> BTreeMap<&'static str, usize> {
        BTreeMap::from([
            ("input_bytes", self.input_bytes),
            ("entities", self.entities),
            ("work", self.work),
            ("records", self.records),
            ("output_bytes", self.output_bytes),
        ])
    }
}

pub struct Incoming {
    pub owner: usize,
    pub path: Vec<usize>,
    pub span: Option<Span>,
    pub alias: Option<Span>,
    pub is_reference: bool,
}
pub struct CodeEntry {
    pub code: Rc<CodeObject>,
    pub definition: usize,
    pub parent: Option<usize>,
    pub incoming: Vec<Incoming>,
}
/// The fixed input and parser evidence are shared by static commands.
pub struct Analysis {
    pub bytes: Vec<u8>,
    pub source_id: String,
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
        let source_id = perf.measure("hash", || rxd_core::identity::content_id(path, &bytes));
        perf.source_id = Some(source_id.clone());
        let mut parse_counts = None;
        let parsed = perf.measure("parse", || -> Result<_> {
            // Only independently verified final CPython formats. A magic identifies
            // a format, not its producer's patch release; raw versions use this
            // same exact allowlist rather than nearest-version selection.
            const FINALS: &[(u8, u16)] = &[(8, 3413), (9, 3425), (10, 3439), (11, 3495), (12, 3531), (13, 3571)];
            let (version, base) = match version {
                Some((3, minor)) => {
                    let magic = FINALS.iter().find(|&&(v, _)| v == minor).map(|&(_, m)| m)
                        .ok_or_else(|| Error::new("INVALID_INPUT", "static PYC queries support verified final CPython 3.8–3.13 formats only"))?;
                    (version_from_magic(magic).unwrap(), 0)
                }
                Some(_) => return Err(Error::new("INVALID_INPUT", "static PYC queries support verified final CPython 3.8–3.13 formats only")),
                None => {
                    let header = PycModule::parse_header(&bytes).map_err(super::input_error)?;
                    if !FINALS.iter().any(|&(_, m)| m == header.magic) || !matches!(header.flags, 0 | 1 | 3) {
                        return Err(Error::new("INVALID_INPUT", "static PYC queries require verified final CPython magic and valid PEP 552 flags"));
                    }
                    (header.version, header.header_size)
                }
            };
            let mut reader = MarshalReader::with_evidence(&bytes[base..], version, base, limits.entities, limits.work);
            let result = reader.load();
            parse_counts = reader.evidence_counts();
            let object = result.map_err(super::input_error)?;
            if !reader.at_eof() { return Err(Error::new("INVALID_INPUT", "trailing marshal data")); }
            if !matches!(&*object, PyObject::Code(_)) { return Err(Error::new("INVALID_INPUT", "top-level object is not a code object")); }
            Ok((version, reader.into_evidence().unwrap()))
        });
        if let Some((entities, work)) = parse_counts {
            perf.counters.entities = Some(entities);
            perf.counters.work = Some(work);
        }
        let (version, evidence) = parsed?;
        let mut analysis = Self {
            bytes,
            source_id,
            version,
            evidence,
            codes: Vec::new(),
            limits,
            code_indexes: HashMap::new(),
        };
        let indexed = perf.measure("visit", || analysis.add_code(0, None).map(|_| ()));
        perf.counters.entities = Some(analysis.evidence.entities);
        perf.counters.work = Some(analysis.evidence.work);
        indexed?;
        Ok(analysis)
    }

    pub fn charge_work(&mut self, amount: usize) -> Result<()> {
        if amount > self.limits.work.saturating_sub(self.evidence.work) {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "static traversal work budget exceeded",
            ));
        }
        self.evidence.work += amount;
        Ok(())
    }
    pub fn parse_code_id(&mut self, id: &str) -> Result<usize> {
        self.charge_work(id.len())?;
        let mut pieces = id.split(':');
        let backend = pieces.next() == Some("rxd1") && pieces.next() == Some("pyc");
        let digest = pieces.next();
        let kind = pieces.next();
        let ordinal = pieces.next().unwrap_or("");
        if !backend
            || digest != Some(self.source_id.as_str())
            || kind != Some("code")
            || pieces.next().is_some()
            || ordinal.is_empty()
            || (ordinal.len() > 1 && ordinal.starts_with('0'))
            || !ordinal.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(Error::new("INVALID_QUERY", "invalid or stale PYC code ID"));
        }
        let index = ordinal
            .parse::<usize>()
            .map_err(|_| Error::new("INVALID_QUERY", "invalid PYC code index"))?;
        if index >= self.codes.len() {
            return Err(Error::new("INVALID_QUERY", "code index does not exist"));
        }
        Ok(index)
    }
    pub fn code_index(&self, definition: usize) -> Option<usize> {
        self.code_indexes.get(&definition).copied()
    }
    pub fn code_id(&self, index: usize) -> String {
        format!("rxd1:pyc:{}:code:{index}", self.source_id)
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
    fn add_code(&mut self, occurrence: usize, parent: Option<usize>) -> Result<usize> {
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
            parent,
            incoming: Vec::new(),
        });
        self.code_indexes.insert(definition, index);
        let consts_occurrence = self.field_occurrence(index, "consts").unwrap();
        let consts_row = &self.evidence.occurrences[consts_occurrence];
        let alias = consts_row.is_reference.then_some(consts_row.span);
        let consts = consts_row.definition;
        self.charge_work(self.evidence.occurrences[consts].children.len())?;
        let children = self.evidence.occurrences[consts].children.clone();
        let mut visited = HashSet::new();
        for (slot, child) in children.into_iter().enumerate() {
            self.walk_constant(index, child, &mut vec![slot], alias, &mut visited)?;
        }
        Ok(index)
    }
    fn walk_constant(
        &mut self,
        owner: usize,
        occurrence: usize,
        path: &mut Vec<usize>,
        alias: Option<Span>,
        visited: &mut HashSet<usize>,
    ) -> Result<()> {
        self.charge_work(1)?;
        let row = &self.evidence.occurrences[occurrence];
        let definition = row.definition;
        let span = row.span;
        let is_reference = row.is_reference;
        let object = self.evidence.occurrences[definition]
            .object
            .as_deref()
            .unwrap();
        if matches!(object, PyObject::Code(_)) {
            let index = self.add_code(occurrence, Some(owner))?;
            // One retained incoming edge per necessary traversal, bounded by cumulative work.
            self.charge_work(path.len() + 1)?;
            self.codes[index].incoming.push(Incoming {
                owner,
                path: path.clone(),
                span: alias.is_none().then_some(span),
                alias,
                is_reference,
            });
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
        let alias = alias.or(is_reference.then_some(span));
        self.charge_work(self.evidence.occurrences[definition].children.len())?;
        let children = self.evidence.occurrences[definition].children.clone();
        for (slot, child) in children.into_iter().enumerate() {
            path.push(slot);
            self.walk_constant(owner, child, path, alias, visited)?;
            path.pop();
        }
        Ok(())
    }
}
