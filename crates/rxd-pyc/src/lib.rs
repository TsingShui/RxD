pub mod code_view;
pub mod references;
pub mod static_analysis;
pub mod string_constants;

use std::path::Path;

use pycdc::{PycError, codegen::generate_limited, decompiler::decompile, loader};
use rxd_core::{Error, Result, perf::Perf};

#[derive(Clone, Copy)]
pub struct Limits {
    pub input_bytes: usize,
    pub work: Option<usize>,
    pub ast_depth: usize,
    pub output_bytes: Option<usize>,
    pub marshal: pycdc::marshal::Limits,
}

pub fn marshal_limits(budget: rxd_core::Budget) -> pycdc::marshal::Limits {
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
    let code = if matches!(error, PycError::ResourceLimit(_)) {
        "RESOURCE_LIMIT"
    } else {
        "INVALID_INPUT"
    };
    Error::new(code, error.to_string())
}

pub fn run(
    path: &Path,
    version: Option<(u8, u8)>,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Decompilation> {
    perf.resource_limits = Some(std::collections::BTreeMap::from([
        ("input_bytes", limits.input_bytes),
        ("ast_depth", limits.ast_depth),
        ("marshal_depth", 128),
        ("marshal_objects", limits.marshal.objects),
        ("marshal_expanded_objects", limits.marshal.expanded_objects),
        ("marshal_copied_bytes", limits.marshal.copied_bytes),
        ("marshal_long_work", 16_777_216),
    ]));
    if let Some(work) = limits.work {
        perf.resource_limits.as_mut().unwrap().insert("work", work);
    }
    if let Some(output_bytes) = limits.output_bytes {
        perf.resource_limits
            .as_mut()
            .unwrap()
            .insert("output_bytes", output_bytes);
    }
    perf.counters.work = Some(0);
    perf.counters.ast_depth = Some(0);
    let bytes = perf.measure("read", || rxd_core::read_bytes(path, limits.input_bytes))?;
    perf.counters.input_bytes = Some(bytes.len());
    let source_id = perf.measure("hash", || rxd_core::identity::input_id(&bytes));
    perf.source_id = Some(source_id.clone());
    let loaded = perf.measure("parse", || {
        loader::load_bytes_with_limits(&bytes, version, limits.marshal).map_err(input_error)
    })?;
    let (outcome, usage) = pycdc::budget::limited(
        pycdc::budget::Limits {
            work: limits.work,
            ast_depth: limits.ast_depth,
        },
        || {
            let result = perf.measure("decompile", || {
                pycdc::budget::cancelled(|| decompile(&loaded.code, loaded.version))
            })?;
            let source = perf.measure("generate", || {
                pycdc::budget::cancelled(|| {
                    generate_limited(
                        &result.body,
                        loaded.version,
                        result.clean,
                        limits.output_bytes,
                    )
                })
            })?;
            Ok((result.clean, source))
        },
    );
    perf.counters.work = Some(usage.work);
    perf.counters.ast_depth = Some(usage.ast_depth);
    let (complete, source) = outcome.map_err(|error| match error {
        PycError::InvalidBytecode { .. } | PycError::ResourceLimit(_) => input_error(error),
        _ => Error::new("DECOMPILE_ERROR", error.to_string()),
    })?;
    Ok(Decompilation {
        source_id,
        input_bytes: bytes.len(),
        version: loaded.version.display(),
        source,
        complete,
    })
}
