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
    let (namespace, local) = match at {
        Some(at) => input.split(at)?,
        None if input.is_archive() => {
            return Err(Error::new(
                "INVALID_QUERY",
                "PYZ decompile requires --at; copy a module or code coordinate from codes",
            ));
        }
        None => (None, "code:0"),
    };
    let index = input::code_index(local)?;
    let data = input.data(namespace, perf)?;
    let version = data.version;
    let member_budget = input.member_budget();
    let limits = Limits {
        input_bytes: budget.input_bytes,
        work: budget.work,
        ast_depth: budget.ast_depth,
        output_bytes: budget.output_bytes,
        marshal: marshal_limits(member_budget),
    };
    record_limits(limits, perf);
    let (complete, source) = recover(data, index, limits, input.initial_work(), perf)?;
    Ok(Decompilation {
        source_id: input.source_id,
        input_bytes: input.input_bytes,
        version: version.display(),
        source,
        complete,
    })
}

fn recover(
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
