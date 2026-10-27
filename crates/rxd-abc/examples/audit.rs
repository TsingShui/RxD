use rxd_abc::{Abc, Decoded, EntityKind, Input, Method, Operand};
use rxd_core::{BudgetPolicy, Error, Result, perf::Perf};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
};

fn missing_strings(abc: &Abc<'_>, decoded: &Decoded<'_>, source: &[u8]) -> Result<Vec<String>> {
    let text = std::str::from_utf8(source)
        .map_err(|_| Error::new("INTERNAL_ERROR", "ABC source is not UTF-8"))?;
    let mut seen = HashSet::new();
    let mut missing = Vec::new();
    for instruction in &decoded.instructions {
        if instruction.opcode.name == "throw.undefinedifholewithname" {
            continue;
        }
        for operand in &instruction.operands {
            let Operand::Id {
                kind: EntityKind::String,
                offset,
                ..
            } = operand
            else {
                continue;
            };
            if !seen.insert(*offset) {
                continue;
            }
            let name = abc.name(*offset)?;
            let mut quoted = Vec::new();
            name.write_quoted(&mut quoted)?;
            let quoted = String::from_utf8(quoted).unwrap_or_default();
            if text.contains(&quoted) {
                continue;
            }
            if let Some(raw) = name.scalar_text()
                && !raw.is_empty()
                && text.contains(raw)
            {
                continue;
            }
            missing.push(name.text.clone());
        }
    }
    Ok(missing)
}

fn trace(abc: &Abc<'_>, method: &Method, decoded: &Decoded<'_>) -> Result<Value> {
    let mut instructions = Vec::new();
    for instruction in &decoded.instructions {
        let mut params = Vec::new();
        for operand in &instruction.operands {
            params.push(match operand {
                Operand::Register(index) => json!({"kind":"reg","index":index}),
                Operand::Unsigned(value) => json!({"kind":"imm","value":value}),
                Operand::Signed(value) => json!({"kind":"imm","value":value}),
                Operand::Float(bits) => {
                    let value = f64::from_bits(*bits);
                    let value = if *bits == (-0.0f64).to_bits() {
                        json!("-0")
                    } else if value.is_nan() {
                        json!("NaN")
                    } else if value == f64::INFINITY {
                        json!("Infinity")
                    } else if value == f64::NEG_INFINITY {
                        json!("-Infinity")
                    } else {
                        json!(value)
                    };
                    json!({"kind":"imm","value":value})
                }
                Operand::Id { kind, offset, .. } => {
                    let label = match kind {
                        EntityKind::String => "STRING",
                        EntityKind::Method => "METHOD",
                        EntityKind::Literal => "LITERALARRAY",
                        EntityKind::Type => "TYPE",
                        EntityKind::Field => "FIELD",
                    };
                    let mut value = json!({"kind":"id","type":label,"offset":offset});
                    if *kind == EntityKind::String {
                        value["units"] = json!(abc.name(*offset)?.units);
                    }
                    value
                }
            });
        }
        instructions.push(json!({"pc":instruction.pc,"size":instruction.opcode.size,
            "opcode":instruction.opcode.code,"name":instruction.opcode.name,"params":params}));
    }
    let mut code_hex = String::new();
    const HEX: &[u8] = b"0123456789abcdef";
    for byte in decoded.code.bytes {
        code_hex.push(HEX[(byte >> 4) as usize] as char);
        code_hex.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(
        json!({"offset":method.offset,"name":method.name.text,"vregs":decoded.code.vregs,"args":decoded.code.args,
        "codeSize":decoded.code.bytes.len(),"codeHex":code_hex,"instructions":instructions,
        "tries":decoded.code.tries.iter().map(|block|json!({"start":block.pc,"length":block.length,
            "catches":block.catches.iter().map(|handler|json!({"type":handler.type_index,"pc":handler.pc,"size":handler.length})).collect::<Vec<_>>()})).collect::<Vec<_>>() }),
    )
}

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let input = arguments.next().map(PathBuf::from).ok_or_else(|| {
        Error::new(
            "INVALID_QUERY",
            "usage: audit INPUT [--entry ENTRY] [--decode] [--traces NEW_DIR] [--source-coverage]",
        )
    })?;
    let mut entry = None;
    let mut decode = false;
    let mut traces: Option<PathBuf> = None;
    let mut source_coverage = false;
    let mut source_path: Option<PathBuf> = None;
    let mut record_source: Option<PathBuf> = None;
    let mut content_check = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--decode" => decode = true,
            "--source-coverage" => source_coverage = true,
            "--content-check" => {
                content_check = true;
                source_coverage = true;
            }
            "--record-source" => {
                record_source = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    Error::new("INVALID_QUERY", "--record-source requires a new directory")
                })?));
            }
            "--source-out" => {
                source_path = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    Error::new("INVALID_QUERY", "--source-out requires a new path")
                })?));
                source_coverage = true;
            }
            "--entry" => {
                entry = Some(
                    arguments
                        .next()
                        .ok_or_else(|| Error::new("INVALID_QUERY", "--entry requires a value"))?,
                )
            }
            "--traces" => {
                traces = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    Error::new("INVALID_QUERY", "--traces requires a value")
                })?));
                decode = true;
            }
            _ => {
                return Err(Error::new(
                    "INVALID_QUERY",
                    format!("unknown audit option {argument}"),
                ));
            }
        }
    }
    if let Some(directory) = &traces {
        fs::create_dir(directory)?;
    }
    let mut perf = Perf::new(false, "analysis", "abc", "audit");
    let input = Input::read(
        &input,
        entry.as_deref(),
        BudgetPolicy::InputScaled,
        &mut perf,
    )?;
    let abc = Abc::parse(&input.bytes, input.budget)?;
    let mut records = Vec::new();
    let (mut method_count, mut code_bytes, mut try_count, mut instruction_count) =
        (0usize, 0usize, 0usize, 0usize);
    let mut histogram = BTreeMap::<&str, usize>::new();
    let mut coverage = BTreeMap::<String, usize>::new();
    let mut failures = Vec::new();
    let mut source_successes = 0usize;
    let mut content_missing_methods = 0usize;
    let mut content_missing = Vec::new();
    let mut source_output = source_path
        .map(|path| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(BufWriter::new)
        })
        .transpose()?;
    let mut module_offsets = HashSet::new();
    for offset in abc.record_offsets() {
        let record = abc.record(offset)?;
        let members = abc.members(&record)?;
        let mut methods = Vec::new();
        let mut record_traces = Vec::new();
        for method in &members.methods {
            method_count += 1;
            let code = method.code_offset.map(|_| abc.code(method)).transpose()?;
            if let Some(code) = &code {
                code_bytes += code.bytes.len();
                try_count += code.tries.len();
            }
            let count = if decode && code.is_some() {
                let decoded = abc.decode(method).map_err(|mut error| {
                    error.message = format!(
                        "record:0x{offset:x} method:0x{:x} {}: {}",
                        method.offset,
                        method.name.quoted(),
                        error.message
                    );
                    error
                })?;
                for instruction in &decoded.instructions {
                    *histogram.entry(instruction.opcode.name).or_default() += 1;
                }
                instruction_count += decoded.instructions.len();
                if traces.is_some() {
                    record_traces.push(trace(&abc, method, &decoded)?);
                }
                Some(decoded.instructions.len())
            } else {
                None
            };
            if source_coverage && code.is_some() {
                match abc.decompile_method(method) {
                    Ok(source) => {
                        source_successes += 1;
                        if content_check {
                            let decoded = abc.decode(method)?;
                            let missing = missing_strings(&abc, &decoded, &source)?;
                            if !missing.is_empty() {
                                content_missing_methods += 1;
                                if content_missing.len() < 20 {
                                    content_missing.push(json!({
                                        "record": offset,
                                        "method": method.offset,
                                        "name": method.name.text,
                                        "missing": missing
                                    }));
                                }
                            }
                        }
                        if let Some(output) = &mut source_output {
                            let source = String::from_utf8(source).map_err(|_| {
                                Error::new("INTERNAL_ERROR", "ABC source is not UTF-8")
                            })?;
                            serde_json::to_writer(
                                &mut *output,
                                &json!({"record":offset,"method":method.offset,"source":source}),
                            )
                            .map_err(|error| Error::new("IO_ERROR", error.to_string()))?;
                            output.write_all(b"\n")?;
                        }
                    }
                    Err(error) => {
                        let kind = error
                            .message
                            .split(" at PC ")
                            .next()
                            .unwrap_or(&error.message);
                        *coverage
                            .entry(format!("{}: {kind}", error.code))
                            .or_default() += 1;
                        failures.push(json!({"method":method.offset,"record":offset,"code":error.code,"error":error.message}));
                    }
                }
            }
            methods.push(json!({"at":format!("method:0x{:x}",method.offset),"name":method.name.text,
                "code_size":code.as_ref().map(|code|code.bytes.len()),"args":code.as_ref().map(|code|code.args),
                "vregs":code.as_ref().map(|code|code.vregs),"instructions":count}));
        }
        if let Some(directory) = &traces {
            fs::write(
                directory.join(format!("{offset:x}.methods.json")),
                serde_json::to_vec(&record_traces)
                    .map_err(|error| Error::new("INTERNAL_ERROR", error.to_string()))?,
            )?;
        }
        let module = abc.module(&record)?;
        if let Some(module) = &module {
            module_offsets.insert(module.offset);
        }
        records.push(json!({"at":format!("record:0x{offset:x}"),"name":record.name.text,"fields":record.fields,
            "methods":methods,"module":module.as_ref().map(|module|json!({"offset":module.offset,"requests":module.requests.len(),
                "imports":module.regular_imports.len()+module.namespace_imports.len(),"local_exports":module.local_exports.len(),
                "indirect_exports":module.indirect_exports.len(),"star_exports":module.star_exports.len()}))}));
    }
    let mut literals = 0usize;
    for offset in abc.literal_offsets() {
        if !module_offsets.contains(&offset) {
            literals += abc.literals(offset)?.len();
        }
    }
    if let Some(output) = &mut source_output {
        output.flush()?;
    }
    let mut record_sources = 0usize;
    let mut record_failures = Vec::new();
    if let Some(directory) = &record_source {
        fs::create_dir_all(directory)?;
        for offset in abc.record_offsets() {
            let record = abc.record(offset)?;
            match abc.decompile_record(&record) {
                Ok(source) => {
                    record_sources += 1;
                    fs::write(directory.join(format!("{offset:x}.js")), source)?;
                }
                Err(error) => record_failures.push(json!({
                    "record": offset,
                    "code": error.code,
                    "error": error.message
                })),
            }
        }
    }
    let report = json!({"source_id":abc.source_id,"records":records,"record_count":records.len(),"method_count":method_count,
        "code_bytes":code_bytes,"tries":try_count,"module_count":module_offsets.len(),"literal_entries":literals,
        "decoded":decode,"instruction_count":instruction_count,"opcode_histogram":histogram,
        "source_coverage":source_coverage,"source_successes":source_successes,"source_errors":coverage,
        "source_failures":failures,"record_sources":record_sources,"record_failures":record_failures,
        "content_missing_methods":content_missing_methods,"content_missing":content_missing});
    let mut output = serde_json::to_vec(&report)
        .map_err(|error| Error::new("INTERNAL_ERROR", error.to_string()))?;
    output.push(b'\n');
    std::io::stdout().lock().write_all(&output)?;
    Ok(())
}
