pub mod input;
mod operands;
pub mod references;
pub mod static_analysis;
pub mod string_constants;

use std::path::Path;

use input::{Data, Input};
use pycdc::{PycError, recovery::Module};
use rxd_core::{Budget, Error, Result, perf::Perf};

#[derive(Clone, Copy)]
pub struct Limits {
    pub input_bytes: usize,
    pub work: Option<usize>,
    pub ast_depth: usize,
    pub output_bytes: Option<usize>,
    pub marshal: pycdc::marshal::Limits,
}

pub fn marshal_limits(budget: Budget) -> pycdc::marshal::Limits {
    pycdc::marshal::Limits {
        objects: budget.semantic_nodes,
        expanded_objects: budget.semantic_nodes,
        copied_bytes: budget.metadata_bytes,
    }
}

pub struct Decompilation {
    pub source_id: String,
    pub input_bytes: usize,
    pub version: String,
    pub source: String,
    pub complete: bool,
    /// One for a selected module/body; every code-bearing module for PYZ all-mode.
    pub modules: usize,
    /// Modules whose decompiler panicked in PYZ all-mode.
    pub failed: Vec<String>,
}

/// One reconstructed module, for callers that write a file per module.
pub struct ModuleDecompilation {
    pub name: String,
    /// Package tree path, e.g. `json/__init__.py` or `json/decoder.py`.
    pub path: String,
    pub source: String,
    pub complete: bool,
    pub failed: Option<String>,
}

fn input_error(error: PycError) -> Error {
    let code = match error {
        PycError::ResourceLimit(_) => "RESOURCE_LIMIT",
        PycError::UnsupportedMagic(_) | PycError::UnsupportedVersion(_) => "UNSUPPORTED_INPUT",
        PycError::CodeNotFound(_) => "NOT_FOUND",
        _ => "INVALID_INPUT",
    };
    Error::new(code, error.to_string())
}

fn record_limits(limits: Limits, perf: &mut Perf) {
    let resource = perf.resource_limits.get_or_insert_with(Default::default);
    resource.extend([
        ("input_bytes", limits.input_bytes),
        ("ast_depth", limits.ast_depth),
        ("marshal_depth", 128),
        ("marshal_objects", limits.marshal.objects),
        ("marshal_expanded_objects", limits.marshal.expanded_objects),
        ("marshal_copied_bytes", limits.marshal.copied_bytes),
        ("marshal_long_work", 16_777_216),
    ]);
    if let Some(work) = limits.work {
        resource.insert("work", work);
    }
    if let Some(bytes) = limits.output_bytes {
        resource.insert("output_bytes", bytes);
    }
}

pub fn run(
    path: &Path,
    version: Option<(u8, u8)>,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Decompilation> {
    record_limits(limits, perf);
    let bytes = perf.measure("read", || rxd_core::read_bytes(path, limits.input_bytes))?;
    let input_bytes = bytes.len();
    perf.counters.input_bytes = Some(input_bytes);
    let source_id = perf.measure("hash", || rxd_core::identity::input_id(&bytes));
    perf.source_id = Some(source_id.clone());
    let (version, header) = pycdc::loader::input_format(&bytes, version).map_err(input_error)?;
    let data = Data {
        bytes,
        version,
        base: header.map_or(0, |header| header.header_size),
        namespace: None,
    };
    let (complete, source) = recover(data, 0, limits, 0, perf)?;
    Ok(Decompilation {
        source_id,
        input_bytes,
        version: version.display(),
        source,
        complete,
        modules: 1,
        failed: Vec::new(),
    })
}

pub fn run_input(
    path: &Path,
    version: Option<(u8, u8)>,
    at: Option<&str>,
    budget: Budget,
    perf: &mut Perf,
) -> Result<Decompilation> {
    let input = Input::load(path, version, budget, perf)?;
    match at {
        Some(at) => {
            let (namespace, local) = input.split(at)?;
            decompile_one(input, namespace, local, budget, perf)
        }
        None if input.is_archive() => decompile_all(input, budget, perf),
        None => decompile_one(input, None, "code:0", budget, perf),
    }
}

/// Reconstruct every code-bearing PYZ module in directory order.
pub fn run_input_modules(
    path: &Path,
    version: Option<(u8, u8)>,
    budget: Budget,
    perf: &mut Perf,
) -> Result<Vec<ModuleDecompilation>> {
    let input = Input::load(path, version, budget, perf)?;
    if !input.is_archive() {
        return Err(Error::new(
            "INVALID_QUERY",
            "per-module output requires a PYZ archive",
        ));
    }
    decompile_modules(&input, budget, perf)
}

fn limits_for(input: &Input, budget: Budget) -> Limits {
    Limits {
        input_bytes: budget.input_bytes,
        work: budget.work,
        ast_depth: budget.ast_depth,
        output_bytes: budget.output_bytes,
        marshal: marshal_limits(input.member_budget()),
    }
}

fn decompile_one(
    input: Input,
    namespace: Option<&str>,
    local: &str,
    budget: Budget,
    perf: &mut Perf,
) -> Result<Decompilation> {
    let index = input::code_index(local)?;
    let data = input.data(namespace, perf)?;
    let version = data.version;
    let limits = limits_for(&input, budget);
    record_limits(limits, perf);
    let (complete, source) = recover(data, index, limits, input.initial_work(), perf)?;
    Ok(Decompilation {
        source_id: input.source_id,
        input_bytes: input.input_bytes,
        version: version.display(),
        source,
        complete,
        modules: 1,
        failed: Vec::new(),
    })
}

/// Reconstruct every code-bearing PYZ module in directory order.
/// Each block is labelled `# ===== <module> =====`; any module failure aborts the call.
/// Mirror PyInstaller's package/module layout, with Python source suffixes.
fn module_source_path(name: &str, package: bool) -> Result<String> {
    if name.is_empty() || name.starts_with('.') || name.contains(['\0', '/', '\\', ':']) {
        return Err(Error::new(
            "INVALID_INPUT",
            format!("unsafe PYZ module name: {name}"),
        ));
    }
    let parts: Vec<&str> = name.split('.').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Err(Error::new(
            "INVALID_INPUT",
            format!("invalid PYZ module name: {name}"),
        ));
    }
    let mut path = String::new();
    if package {
        for part in &parts {
            path.push_str(part);
            path.push('/');
        }
        path.push_str("__init__.py");
    } else {
        for part in &parts[..parts.len() - 1] {
            path.push_str(part);
            path.push('/');
        }
        path.push_str(parts[parts.len() - 1]);
        path.push_str(".py");
    }
    Ok(path)
}

fn decompile_modules(
    input: &Input,
    budget: Budget,
    perf: &mut Perf,
) -> Result<Vec<ModuleDecompilation>> {
    let entries: Vec<(String, bool)> = input
        .code_modules()
        .into_iter()
        .map(|(name, package)| (name.to_owned(), package))
        .collect();
    if entries.is_empty() {
        return Err(Error::new("NOT_FOUND", "PYZ has no code-bearing modules"));
    }
    let limits = limits_for(input, budget);
    record_limits(limits, perf);
    let mut work = input.initial_work();
    let mut modules = Vec::new();
    for (name, package) in entries {
        let path = module_source_path(&name, package)?;
        let data = input.data(Some(&name), perf)?;
        let (source, complete, failed) = match recover(data, 0, limits, work, perf) {
            Ok((module_complete, module_source)) => (module_source, module_complete, None),
            Err(error) if error.code == "DECOMPILE_ERROR" => {
                let message = error.message;
                (
                    format!("# PYC_FALLBACK: {message}: module body unavailable\n"),
                    false,
                    Some(message),
                )
            }
            Err(error) => return Err(error),
        };
        work = perf.counters.work.unwrap_or(work);
        modules.push(ModuleDecompilation {
            name,
            path,
            source,
            complete,
            failed,
        });
    }
    Ok(modules)
}

fn decompile_all(input: Input, budget: Budget, perf: &mut Perf) -> Result<Decompilation> {
    let modules = decompile_modules(&input, budget, perf)?;
    let mut source = String::new();
    let mut complete = true;
    let mut failed = Vec::new();
    for module in &modules {
        complete &= module.complete;
        if module.failed.is_some() {
            failed.push(module.name.clone());
        }
        if !source.is_empty() {
            source.push('\n');
        }
        use std::fmt::Write;
        writeln!(source, "# ===== {} =====", module.name).expect("string write");
        source.push_str(&module.source);
        if !module.source.ends_with('\n') {
            source.push('\n');
        }
        if budget
            .output_bytes
            .is_some_and(|limit| source.len() > limit)
        {
            return Err(Error::failure(
                rxd_core::Failure::Resource,
                "serialized PYC output bytes",
            ));
        }
    }
    let version = input.version();
    Ok(Decompilation {
        source_id: input.source_id,
        input_bytes: input.input_bytes,
        version: version.display(),
        source,
        complete,
        modules: modules.len(),
        failed,
    })
}

fn recover(
    data: Data,
    index: usize,
    limits: Limits,
    initial_work: usize,
    perf: &mut Perf,
) -> Result<(bool, String)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        recover_inner(data, index, limits, initial_work, perf)
    }))
    .map_err(|_| {
        Error::new(
            "DECOMPILE_ERROR",
            "pycdc panicked during module reconstruction",
        )
    })?
}

fn recover_inner(
    data: Data,
    index: usize,
    limits: Limits,
    initial_work: usize,
    perf: &mut Perf,
) -> Result<(bool, String)> {
    perf.counters.work = Some(initial_work);
    perf.counters.ast_depth = Some(0);
    let code = perf.measure("parse", || {
        pycdc::PycModule::unmarshal_with_limits(
            &data.bytes[data.base..],
            data.version,
            limits.marshal,
        )
        .map_err(input_error)
    })?;
    let (outcome, usage) = pycdc::budget::limited(
        pycdc::budget::Limits {
            work: limits.work.map(|limit| limit.saturating_sub(initial_work)),
            ast_depth: limits.ast_depth,
        },
        || {
            let (module, result) = perf.measure("decompile", || {
                pycdc::budget::cancelled(|| {
                    let module = Module::new(&code).named(data.namespace);
                    let result = module.decompile_at(index, data.version)?;
                    Ok((module, result))
                })
            })?;
            let source = perf.measure("generate", || {
                pycdc::budget::cancelled(|| {
                    module.generate_at(&result.body, data.version, limits.output_bytes, index)
                })
            })?;
            Ok((module.complete_at(index), source))
        },
    );
    perf.counters.work = Some(initial_work.saturating_add(usage.work));
    perf.counters.ast_depth = Some(usage.ast_depth);
    outcome.map_err(|error| match error {
        PycError::InvalidBytecode { .. }
        | PycError::ResourceLimit(_)
        | PycError::UnsupportedMagic(_)
        | PycError::UnsupportedVersion(_)
        | PycError::CodeNotFound(_) => input_error(error),
        _ => Error::new("DECOMPILE_ERROR", error.to_string()),
    })
}
