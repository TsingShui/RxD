pub mod code_view;
pub mod references;
pub mod static_analysis;
pub mod string_constants;

use std::path::Path;

use pycdc::{PycError, codegen::generate_limited, decompiler::decompile, loader};
use rxd_core::{Error, Result, perf::Perf};

pub struct Limits {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub work: usize,
    pub ast_depth: usize,
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
        ("max_input_bytes", limits.input_bytes),
        ("max_output_bytes", limits.output_bytes),
        ("max_work", limits.work),
        ("max_ast_depth", limits.ast_depth),
        ("marshal_depth", 128),
        ("marshal_objects", 1_000_000),
        ("marshal_expanded_objects", 1_000_000),
        ("marshal_copied_bytes", 128 * 1024 * 1024),
        ("marshal_long_work", 16_777_216),
    ]));
    perf.counters.work = Some(0);
    perf.counters.ast_depth = Some(0);
    let bytes = perf.measure("read", || rxd_core::read_bytes(path, limits.input_bytes))?;
    perf.counters.input_bytes = Some(bytes.len());
    let source_id = perf.measure("hash", || rxd_core::identity::content_id(path, &bytes));
    perf.source_id = Some(source_id.clone());
    let loaded = perf.measure("parse", || {
        loader::load_bytes(&bytes, version).map_err(input_error)
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
