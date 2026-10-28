use rxd_ark::{Abc, Input, ReconstructionWarning};
use rxd_core::{BudgetPolicy, Error, Result, perf::Perf};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
};

fn write_warnings(warnings: &[ReconstructionWarning]) -> Result<()> {
    let mut stderr = std::io::stderr().lock();
    for warning in warnings {
        writeln!(stderr, "warning[{}]: {}", warning.code(), warning.message())?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or_else(|| {
        Error::new(
            "INVALID_QUERY",
            "audit INPUT [--at method:0x..] [--sources NEW_JSONL]",
        )
    })?);
    let mut at = None;
    let mut destination = None;
    let mut record_directory = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--at" => {
                at = Some(
                    args.next()
                        .ok_or_else(|| Error::new("INVALID_QUERY", "--at requires a coordinate"))?,
                )
            }
            "--sources" => {
                destination = Some(PathBuf::from(args.next().ok_or_else(|| {
                    Error::new("INVALID_QUERY", "--sources requires a new JSONL path")
                })?))
            }
            "--records" => {
                record_directory = Some(PathBuf::from(args.next().ok_or_else(|| {
                    Error::new("INVALID_QUERY", "--records requires a new directory")
                })?))
            }
            _ => {
                return Err(Error::new(
                    "INVALID_QUERY",
                    format!("unknown argument {argument}"),
                ));
            }
        }
    }
    let mut perf = Perf::new(false, "analysis", "abc", "ark-ir-audit");
    let input = Input::read(&path, None, BudgetPolicy::InputScaled, &mut perf)?;
    let abc = Abc::parse(&input.bytes, input.budget)?;
    if let Some(at) = at {
        let reconstructed = abc.decompile_target(rxd_ark::Coordinate::parse(&at)?)?;
        write_warnings(reconstructed.warnings)?;
        std::io::stdout().lock().write_all(&reconstructed.source)?;
        return Ok(());
    }
    let mut output = destination
        .map(|path| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(BufWriter::new)
        })
        .transpose()?;
    if let Some(directory) = &record_directory {
        fs::create_dir(directory)?;
    }
    let mut record_count = 0;
    let mut source_records = 0;
    let mut record_warning_offsets = Vec::new();
    let mut method_warnings = 0;
    let mut record_failures = Vec::new();
    let mut total = 0;
    let mut successes = 0;
    let mut bytes = 0;
    let mut blocks = 0;
    let mut residual = 0;
    let mut structured_tries = 0;
    let mut values = 0;
    let mut eliminated = 0;
    let mut errors = BTreeMap::<String, usize>::new();
    let mut failures = Vec::new();
    for offset in abc.record_offsets() {
        let record = abc.record(offset)?;
        let members = abc.members(&record)?;
        record_count += 1;
        if let Some(directory) = &record_directory
            && abc.module(&record)?.is_some()
        {
            match abc.decompile_record(&record) {
                Ok(reconstructed) => {
                    if !reconstructed.warnings.is_empty() {
                        record_warning_offsets.try_reserve(1).map_err(|_| {
                            Error::new("RESOURCE_LIMIT", "Ark warning index allocation failed")
                        })?;
                        record_warning_offsets.push(offset);
                    }
                    fs::write(
                        directory.join(format!("{offset:x}.js")),
                        reconstructed.source,
                    )?;
                    source_records += 1;
                }
                Err(error) => record_failures
                    .push(json!({"record":offset,"code":error.code,"error":error.message})),
            }
        }
        for method in &members.methods {
            if method.code_offset.is_none() {
                continue;
            }
            total += 1;
            match abc.reconstruct(method) {
                Ok(reconstructed) => {
                    successes += 1;
                    if !reconstructed.warnings.is_empty() {
                        method_warnings += 1;
                    }
                    bytes += reconstructed.source.len();
                    blocks += reconstructed.stats.blocks;
                    residual += reconstructed.stats.residual_blocks;
                    structured_tries += reconstructed.stats.structured_tries;
                    values += reconstructed.stats.values;
                    eliminated += reconstructed.stats.eliminated_values;
                    if let Some(output) = &mut output {
                        let source = String::from_utf8(reconstructed.source).map_err(|_| {
                            Error::new("INTERNAL_ERROR", "Ark AST codegen produced invalid UTF-8")
                        })?;
                        let mut row = json!({"record":offset,"method":method.offset,"name":method.name.text,"source":source});
                        if !reconstructed.warnings.is_empty() {
                            row["warnings"] = json!(
                                reconstructed
                                    .warnings
                                    .iter()
                                    .map(|warning| warning.code())
                                    .collect::<Vec<_>>()
                            );
                        }
                        serde_json::to_writer(&mut *output, &row)
                            .map_err(|error| Error::new("IO_ERROR", error.to_string()))?;
                        output.write_all(b"\n")?;
                    }
                }
                Err(error) => {
                    let group = error
                        .message
                        .rsplit(": ")
                        .next()
                        .unwrap_or(&error.message)
                        .to_owned();
                    *errors.entry(group).or_default() += 1;
                    failures.push(json!({"record":offset,"method":method.offset,"name":method.name.text,"code":error.code,"error":error.message}));
                }
            }
        }
    }
    if let Some(output) = &mut output {
        output.flush()?;
    }
    if method_warnings != 0 || !record_warning_offsets.is_empty() {
        write_warnings(&[ReconstructionWarning::IteratorRuntimeSemantics])?;
    }
    println!(
        "{}",
        json!({"source_id":abc.source_id,"records":record_count,"source_records":source_records,"record_warnings":record_warning_offsets.len(),"record_warning_offsets":record_warning_offsets,"record_failures":record_failures,"methods":total,"successes":successes,"method_warnings":method_warnings,"source_bytes":bytes,
        "blocks":blocks,"residual_blocks":residual,"structured_tries":structured_tries,"values":values,"eliminated_values":eliminated,"errors":errors,"failures":failures})
    );
    Ok(())
}
