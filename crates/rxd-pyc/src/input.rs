//! One fixed PYC/PYZ snapshot and lazy, module-scoped analysis.
use std::path::Path;

use pycdc::version::PythonVersion;
use rxd_core::{Budget, Error, Result, perf::Perf};
use rxd_pyinstaller::pyz::Archive;

use crate::static_analysis::{Analysis, Limits};

enum Format {
    Single {
        bytes: Vec<u8>,
        version: PythonVersion,
        base: usize,
    },
    Pyz {
        archive: Archive,
        version: PythonVersion,
    },
}

pub struct Input {
    format: Format,
    pub source_id: String,
    pub input_bytes: usize,
    pub budget: Budget,
}

pub(crate) struct Data {
    pub bytes: Vec<u8>,
    pub version: PythonVersion,
    pub base: usize,
    pub namespace: Option<String>,
}

impl Input {
    pub fn load(
        path: &Path,
        version: Option<(u8, u8)>,
        budget: Budget,
        perf: &mut Perf,
    ) -> Result<Self> {
        let limits = Self::limits(budget);
        perf.resource_limits = Some(limits.perf_limits());
        if let Some(cap) = budget.entry_bytes {
            perf.resource_limits
                .as_mut()
                .unwrap()
                .insert("entry_bytes", cap);
        }
        if let Some(cap) = budget.output_bytes {
            perf.resource_limits
                .as_mut()
                .unwrap()
                .insert("output_bytes", cap);
        }
        let bytes = perf.measure("read", || rxd_core::read_bytes(path, budget.input_bytes))?;
        let input_bytes = bytes.len();
        perf.counters.input_bytes = Some(input_bytes);
        let source_id = perf.measure("hash", || rxd_core::identity::input_id(&bytes));
        perf.source_id = Some(source_id.clone());
        let format = perf.measure("parse", || -> Result<_> {
            if bytes.starts_with(b"PYZ\0") {
                if version.is_some() {
                    return Err(Error::new(
                        "INVALID_QUERY",
                        "--version is for raw marshal, not PYZ",
                    ));
                }
                let archive = Archive::parse(bytes, budget)?;
                let magic = archive.magic();
                let magic = u16::from_le_bytes([magic[0], magic[1]]);
                let version = pycdc::version::version_from_magic(magic)
                    .ok_or_else(|| crate::input_error(pycdc::PycError::UnsupportedMagic(magic)))?;
                Ok(Format::Pyz { archive, version })
            } else {
                if version.is_none() && bytes.starts_with(b"PK\x03\x04") {
                    return Err(Error::new(
                        "UNSUPPORTED_INPUT",
                        "ZIP zipapps are not PyInstaller PYZ archives",
                    ));
                }
                let (version, header) =
                    pycdc::loader::input_format(&bytes, version).map_err(crate::input_error)?;
                Ok(Format::Single {
                    bytes,
                    version,
                    base: header.map_or(0, |header| header.header_size),
                })
            }
        })?;
        if let Format::Pyz { archive, .. } = &format {
            perf.extra(
                "modules",
                archive
                    .entries()
                    .iter()
                    .filter(|entry| entry.kind.has_code())
                    .count(),
            );
        }
        Ok(Self {
            format,
            source_id,
            input_bytes,
            budget,
        })
    }

    fn limits(budget: Budget) -> Limits {
        Limits {
            input_bytes: budget.input_bytes,
            entities: budget.semantic_nodes,
            metadata_bytes: budget.metadata_bytes,
            marshal: crate::marshal_limits(budget),
            work: budget.work,
        }
    }

    pub fn is_archive(&self) -> bool {
        matches!(self.format, Format::Pyz { .. })
    }

    /// Code-bearing PYZ module names in directory order; empty for standalone PYC.
    pub fn code_modules(&self) -> Vec<&str> {
        match &self.format {
            Format::Single { .. } => Vec::new(),
            Format::Pyz { archive, .. } => archive
                .entries()
                .iter()
                .filter(|entry| entry.kind.has_code())
                .map(|entry| entry.name.as_str())
                .collect(),
        }
    }

    pub fn version(&self) -> PythonVersion {
        match &self.format {
            Format::Single { version, .. } | Format::Pyz { version, .. } => *version,
        }
    }

    /// Module inventory uses the PYZ directory; standalone PYC validates its one body.
    pub fn modules(
        &self,
        filter: Option<&rxd_core::filter::Contains>,
        perf: &mut Perf,
        mut emit: impl FnMut(&str, &'static str) -> Result<()>,
    ) -> Result<()> {
        let Format::Pyz { archive, .. } = &self.format else {
            return self.visit(None, perf, |analysis, _| {
                let at = analysis.code_id(0);
                analysis.charge_work(at.len())?;
                if filter.is_none_or(|filter| filter.matches(&at)) {
                    emit(&at, "module")?;
                }
                Ok(())
            });
        };
        let mut work = self.initial_work();
        let result = perf.measure("query", || {
            for entry in archive.entries() {
                let amount = entry.name.len().saturating_add(1);
                if self
                    .budget
                    .work
                    .is_some_and(|limit| amount > limit.saturating_sub(work))
                {
                    return Err(Error::new(
                        "RESOURCE_LIMIT",
                        "PYZ module listing work exceeded",
                    ));
                }
                work = work.checked_add(amount).ok_or_else(|| {
                    Error::new("RESOURCE_LIMIT", "PYZ module listing work overflow")
                })?;
                let kind = match entry.kind {
                    rxd_pyinstaller::pyz::Kind::Module => "module",
                    rxd_pyinstaller::pyz::Kind::Package => "package",
                    rxd_pyinstaller::pyz::Kind::Namespace => "namespace",
                    rxd_pyinstaller::pyz::Kind::Data => continue,
                };
                if filter.is_none_or(|filter| filter.matches(&entry.name)) {
                    emit(&entry.name, kind)?;
                }
            }
            Ok(())
        });
        perf.counters.work = Some(work);
        result
    }

    /// An exact module scope, not a substring or a nested code-body selector.
    pub fn module_scope<'a>(&self, at: &'a str) -> Result<Option<&'a str>> {
        let (namespace, local) = self.split(at)?;
        if local != "code:0" {
            return Err(Error::new(
                "INVALID_QUERY",
                "--within requires a module coordinate, not a nested code coordinate",
            ));
        }
        Ok(namespace)
    }

    pub(crate) fn member_budget(&self) -> Budget {
        let mut budget = self.budget;
        if let Format::Pyz { archive, .. } = &self.format {
            budget.metadata_bytes = budget
                .metadata_bytes
                .saturating_sub(archive.metadata_bytes());
            budget.semantic_nodes = budget
                .semantic_nodes
                .saturating_sub(archive.entries().len());
        }
        budget
    }

    pub fn initial_work(&self) -> usize {
        match &self.format {
            Format::Single { .. } => 0,
            Format::Pyz { archive, .. } => archive.work(),
        }
    }

    /// Return a validated module selector and its original member-local coordinate.
    pub fn split<'a>(&self, at: &'a str) -> Result<(Option<&'a str>, &'a str)> {
        match &self.format {
            Format::Single { .. } => {
                if at.contains("::") {
                    return Err(Error::new(
                        "INVALID_QUERY",
                        "standalone PYC coordinates have no module prefix",
                    ));
                }
                Ok((None, at))
            }
            Format::Pyz { archive, .. } => {
                let (name, local) = at.split_once("::").unwrap_or((at, "code:0"));
                let index = archive
                    .find(name)
                    .ok_or_else(|| Error::new("NOT_FOUND", format!("no PYZ module {name:?}")))?;
                if !archive.entries()[index].kind.has_code() {
                    return Err(Error::new(
                        "UNSUPPORTED_INPUT",
                        format!("PYZ entry {name:?} has no Python code body"),
                    ));
                }
                if at.contains("::") && local == "code:0" {
                    return Err(Error::new(
                        "INVALID_QUERY",
                        format!("select the whole module with {name:?}, without ::code:0"),
                    ));
                }
                Ok((Some(name), local))
            }
        }
    }

    pub(crate) fn data(&self, namespace: Option<&str>, perf: &mut Perf) -> Result<Data> {
        match &self.format {
            Format::Single {
                bytes,
                version,
                base,
            } => {
                if namespace.is_some() {
                    return Err(Error::new(
                        "INVALID_QUERY",
                        "PYC input has no archive modules",
                    ));
                }
                let mut copy = Vec::new();
                copy.try_reserve_exact(bytes.len())
                    .map_err(|_| Error::new("RESOURCE_LIMIT", "PYC snapshot allocation failed"))?;
                copy.extend_from_slice(bytes);
                Ok(Data {
                    bytes: copy,
                    version: *version,
                    base: *base,
                    namespace: None,
                })
            }
            Format::Pyz { archive, version } => {
                let namespace = namespace.ok_or_else(|| Error::new("INVALID_QUERY", "PYZ reconstruction requires --at; copy a module or code coordinate from codes"))?;
                let index = archive.find(namespace).ok_or_else(|| {
                    Error::new("NOT_FOUND", format!("no PYZ module {namespace:?}"))
                })?;
                if !archive.entries()[index].kind.has_code() {
                    return Err(Error::new(
                        "UNSUPPORTED_INPUT",
                        "selected PYZ entry has no code body",
                    ));
                }
                let bytes = perf
                    .measure("inflate", || archive.read(index, self.budget.entry_bytes))
                    .map_err(|error| {
                        Error::new(error.code, format!("{namespace}: {}", error.message))
                    })?;
                Ok(Data {
                    bytes,
                    version: *version,
                    base: 0,
                    namespace: Some(namespace.to_owned()),
                })
            }
        }
    }

    /// Analyze one member at a time; explicit invocation work remains cumulative.
    pub fn visit(
        &self,
        namespace: Option<&str>,
        perf: &mut Perf,
        mut query: impl FnMut(&mut Analysis, &mut Perf) -> Result<()>,
    ) -> Result<()> {
        let mut work = self.initial_work();
        let mut entities = 0usize;
        let mut visit = |namespace| -> Result<()> {
            let data = self.data(namespace, perf)?;
            let mut limits = Self::limits(self.member_budget());
            limits.work = limits.work.map(|limit| limit.saturating_sub(work));
            let parsed = Analysis::from_bytes(
                data.bytes,
                data.version,
                data.base,
                data.namespace,
                self.source_id.clone(),
                limits,
                perf,
            );
            let mut analysis = match parsed {
                Ok(analysis) => analysis,
                Err(error) => {
                    perf.counters.work = Some(work.saturating_add(perf.counters.work.unwrap_or(0)));
                    perf.counters.entities =
                        Some(entities.saturating_add(perf.counters.entities.unwrap_or(0)));
                    return Err(if let Some(namespace) = namespace {
                        Error::new(error.code, format!("{namespace}: {}", error.message))
                    } else {
                        error
                    });
                }
            };
            let result = query(&mut analysis, perf);
            work = work
                .checked_add(analysis.evidence.work)
                .ok_or_else(|| Error::new("RESOURCE_LIMIT", "PYC invocation work overflow"))?;
            entities = entities
                .checked_add(analysis.evidence.entities)
                .ok_or_else(|| Error::new("RESOURCE_LIMIT", "PYC entity counter overflow"))?;
            perf.counters.work = Some(work);
            perf.counters.entities = Some(entities);
            result
        };
        match &self.format {
            Format::Single { .. } => visit(namespace),
            Format::Pyz { archive, .. } => {
                if let Some(namespace) = namespace {
                    return visit(Some(namespace));
                }
                for entry in archive
                    .entries()
                    .iter()
                    .filter(|entry| entry.kind.has_code())
                {
                    visit(Some(&entry.name))?;
                }
                perf.counters.work = Some(work);
                perf.counters.entities = Some(entities);
                Ok(())
            }
        }
    }
}

pub(crate) fn code_index(id: &str) -> Result<usize> {
    let ordinal = id
        .strip_prefix("code:")
        .ok_or_else(|| Error::new("INVALID_QUERY", "expected a code coordinate"))?;
    if ordinal.is_empty()
        || ordinal.len() > 1 && ordinal.starts_with('0')
        || !ordinal.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Error::new("INVALID_QUERY", "malformed PYC code coordinate"));
    }
    ordinal
        .parse()
        .map_err(|_| Error::new("INVALID_QUERY", "PYC code index overflow"))
}
