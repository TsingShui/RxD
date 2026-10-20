use super::*;
use std::io::Read;

const VERSIONS: &[(&str, &str)] = &[
    ("3.8.20", "3.8"),
    ("3.9.6", "3.9"),
    ("3.10.16", "3.10"),
    ("3.11.11", "3.11"),
    ("3.12.14", "3.12"),
    ("3.13.2", "3.13"),
];
fn corpus(version: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/matrix")
        .join(version)
        .join(name)
}
fn oracle(version: &str, label: &str) -> Value {
    let data = fs::read(corpus(version, &format!("{label}.oracle.json.gz"))).unwrap();
    let mut unpacked = Vec::new();
    flate2::read::GzDecoder::new(&data[..])
        .read_to_end(&mut unpacked)
        .unwrap();
    serde_json::from_slice(&unpacked).unwrap()
}
fn command(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
fn rows(output: &Output) -> Vec<Value> {
    success(output);
    output
        .stdout
        .split(|b| *b == b'\n')
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::from_slice(s).unwrap())
        .collect()
}
#[test]
fn pyc_final_versions_enumerate_the_independent_graph_in_every_header_mode() {
    for &(version, minor) in VERSIONS {
        let expected = oracle(version, "graph");
        for mode in [
            "timestamp.pyc",
            "hash-unchecked.pyc",
            "hash-checked.pyc",
            "marshal",
        ] {
            let path = corpus(version, &format!("graph.{mode}"));
            let args = if mode == "marshal" {
                vec!["--version", minor]
            } else {
                Vec::new()
            };
            let output = command("codes", &path, &args);
            let all = rows(&output);
            let codes = &all[1..all.len() - 1];
            assert_eq!(codes.len(), expected["codes"].as_array().unwrap().len());
            assert_eq!(all.last().unwrap()["complete"], true);
            for (index, (actual, code)) in codes
                .iter()
                .zip(expected["codes"].as_array().unwrap())
                .enumerate()
            {
                assert_eq!(actual["at"], format!("code:{index}"));
                assert_eq!(actual["detail"]["name"], code["name"]);
                assert_js_keys(
                    &actual["detail"]["fields"],
                    if matches!(minor, "3.8" | "3.9" | "3.10") {
                        &[
                            "arg_count",
                            "posonly_arg_count",
                            "kwonly_arg_count",
                            "n_locals",
                            "stack_size",
                            "flags",
                            "code",
                            "consts",
                            "names",
                            "varnames",
                            "freevars",
                            "cellvars",
                            "filename",
                            "name",
                            "first_line",
                            "line_table",
                        ]
                    } else {
                        &[
                            "arg_count",
                            "posonly_arg_count",
                            "kwonly_arg_count",
                            "stack_size",
                            "flags",
                            "code",
                            "consts",
                            "names",
                            "localsplus_names",
                            "localsplus_kinds",
                            "filename",
                            "name",
                            "qualname",
                            "first_line",
                            "line_table",
                            "exception_table",
                        ]
                    },
                );
                assert_eq!(
                    actual["detail"]["localsplus_count"].is_null(),
                    matches!(minor, "3.8" | "3.9" | "3.10")
                );

                assert_eq!(actual["detail"]["qualname"], code["qualname"]);
                assert_eq!(actual["detail"]["arg_count"], code["argcount"]);
                assert_eq!(
                    actual["detail"]["bytecode_bytes"],
                    code["code_hex"].as_str().unwrap().len() / 2
                );
                let range = &expected["marshal_ranges"]["code_ranges"][index];
                let prefix = if mode == "marshal" {
                    "marshal"
                } else {
                    "pyc_marshal"
                };
                assert_eq!(actual["start"], range[format!("{prefix}_start")]);
                assert_eq!(actual["end"], range[format!("{prefix}_end")]);
            }
        }
    }
}

fn code_rows(version: &str, label: &str) -> Vec<Value> {
    rows(&command(
        "codes",
        &corpus(version, &format!("{label}.timestamp.pyc")),
        &[],
    ))
    .into_iter()
    .filter(|r| r["record"] == "code")
    .collect()
}
fn view(path: &Path, at: &str) -> Value {
    let output = command("get-code", path, &["--at", at]);
    success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn pyc_final_versions_fetch_original_operands_tables_lines_exceptions_and_cache_units() {
    for &(version, minor) in VERSIONS {
        for label in ["graph", "extended", "semantics"] {
            let path = corpus(version, &format!("{label}.timestamp.pyc"));
            let expected = oracle(version, label);
            let codes = code_rows(version, label);
            for (index, (row, code)) in codes
                .iter()
                .zip(expected["codes"].as_array().unwrap())
                .enumerate()
            {
                let actual = view(&path, row["at"].as_str().unwrap());
                let d = &actual["detail"];
                assert_eq!(
                    d["bytecode_hex"], code["code_hex"],
                    "{version}/{label}/{index}"
                );
                assert_eq!(d["complete"], true);
                assert_eq!(d["metadata"], row["detail"]);
                let units = d["instructions"].as_array().unwrap();
                assert_eq!(units.len(), code["code_hex"].as_str().unwrap().len() / 4);
                for ins in code["dis_instructions"].as_array().unwrap() {
                    let unit = units.iter().find(|u| u["offset"] == ins["offset"]).unwrap();
                    assert_eq!(unit["name"], ins["opname"], "{version}/{label}/{index}");
                    assert_eq!(unit["raw_operand"], ins["raw_arg_byte"]);
                    if unit["role"] != "instruction" {
                        continue;
                    }
                    assert_eq!(
                        unit["operand"], ins["arg"],
                        "{version}/{label}/{index}/{}",
                        ins["offset"]
                    );
                    if !unit["jump_target"].is_null() {
                        assert_eq!(unit["jump_target"], ins["argval"]["value"]);
                    }
                    let start = unit["start"].as_u64().unwrap() as usize;
                    let end = unit["end"].as_u64().unwrap() as usize;
                    assert_eq!(end - start, 2);
                    let bytecode_start =
                        expected["marshal_ranges"]["code_ranges"][index]["pyc_bytecode_start"]
                            .as_u64()
                            .unwrap() as usize;
                    assert_eq!(
                        start,
                        bytecode_start + ins["offset"].as_u64().unwrap() as usize
                    );
                    let targets = unit["targets"].as_array().unwrap();
                    if !targets.is_empty() && targets[0]["table"] != "consts" {
                        let names: Vec<Value> = targets
                            .iter()
                            .map(|t| {
                                d["values"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .find(|v| v["id"] == t["value_id"])
                                    .unwrap()["value"]
                                    .clone()
                            })
                            .collect();
                        let argval = &ins["argval"];
                        if names.len() == 2 {
                            assert_eq!(
                                names,
                                argval["items"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|i| i["value"].clone())
                                    .collect::<Vec<_>>()
                            );
                        } else {
                            assert_eq!(
                                names[0], argval["value"],
                                "{version}/{label}/{index}/{}",
                                ins["offset"]
                            );
                        }
                    }
                }
                for cache in code["cache_units"].as_array().unwrap() {
                    let unit = units
                        .iter()
                        .find(|u| u["offset"] == cache["offset"])
                        .unwrap();
                    assert_eq!(unit["role"], "cache");
                    assert_eq!(unit["owner_offset"], cache["owner_offset"]);
                    assert_eq!(unit["raw_hex"], cache["raw_hex"]);
                }
                for table in ["consts", "names", "varnames", "freevars", "cellvars"] {
                    assert_eq!(
                        d["tables"][table]["entries"].as_array().unwrap().len(),
                        code[table].as_array().unwrap().len()
                    );
                }
                assert_eq!(
                    d["tables"]["localsplus"]["present"],
                    !["3.8", "3.9", "3.10"].contains(&minor)
                );
                let exceptions: Vec<Value> = d["exceptions"].as_array().unwrap().iter().map(|e| serde_json::json!({"start":e["start"],"end":e["end"],"target":e["target"],"depth":e["depth"],"lasti":e["lasti"]})).collect();
                assert_eq!(exceptions, *code["exceptions"].as_array().unwrap());
                if let Some(ranges) = code["line_ranges"].as_array() {
                    // co_lines in 3.12+ merges consecutive records with the same
                    // line even when columns differ. FETCH keeps original entries.
                    for unit in units {
                        let offset = unit["offset"].as_u64().unwrap();
                        let expected_line = ranges
                            .iter()
                            .find(|r| {
                                r[0].as_u64().unwrap() <= offset && r[1].as_u64().unwrap() > offset
                            })
                            .unwrap();
                        assert_eq!(
                            unit["line"], expected_line[2],
                            "{version}/{label}/{index}/{offset}"
                        );
                    }
                } else {
                    for unit in units {
                        let line = code["line_starts"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .rev()
                            .find(|l| l[0].as_u64().unwrap() <= unit["offset"].as_u64().unwrap())
                            .unwrap();
                        assert_eq!(
                            unit["line"], line[1],
                            "{version}/{label}/{index}/{}",
                            unit["offset"]
                        );
                    }
                }
                if let Some(positions) = code["positions"].as_array() {
                    for unit in units {
                        let pos = &positions[unit["offset"].as_u64().unwrap() as usize / 2];
                        let loc = d["line_mapping"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|l| {
                                l["start"].as_u64().unwrap() <= unit["offset"].as_u64().unwrap()
                                    && l["end"].as_u64().unwrap() > unit["offset"].as_u64().unwrap()
                            })
                            .unwrap();
                        assert_eq!(
                            serde_json::json!([
                                loc["line"],
                                loc["end_line"],
                                loc["column"],
                                loc["end_column"]
                            ]),
                            *pos
                        );
                    }
                }
            }
        }
    }
}

fn name_operation(op: &str, position: usize) -> Option<&'static str> {
    Some(match op {
        "LOAD_NAME" => "name_read",
        "STORE_NAME" => "name_write",
        "DELETE_NAME" => "name_delete",
        "LOAD_GLOBAL" => "global_read",
        "STORE_GLOBAL" => "global_write",
        "DELETE_GLOBAL" => "global_delete",
        "LOAD_FAST" | "LOAD_FAST_CHECK" | "LOAD_FAST_LOAD_FAST" => "local_read",
        "STORE_FAST" | "STORE_FAST_STORE_FAST" => "local_write",
        "STORE_FAST_LOAD_FAST" if position == 0 => "local_write",
        "STORE_FAST_LOAD_FAST" => "local_read",
        "DELETE_FAST" => "local_delete",
        "LOAD_FAST_AND_CLEAR" => "local_read_clear",
        "MAKE_CELL" => "cell_create",
        "LOAD_CLOSURE" => "closure_capture",
        "LOAD_DEREF" => "closure_read",
        "STORE_DEREF" => "closure_write",
        "DELETE_DEREF" => "closure_delete",
        "LOAD_CLASSDEREF" => "class_closure_read",
        "LOAD_ATTR" | "LOAD_SUPER_ATTR" | "LOAD_METHOD" => "attribute_read",
        "STORE_ATTR" => "attribute_write",
        "DELETE_ATTR" => "attribute_delete",
        "IMPORT_NAME" => "import_name",
        "IMPORT_FROM" => "import_from",
        "LOAD_FROM_DICT_OR_GLOBALS" => "dict_or_global_read",
        "LOAD_FROM_DICT_OR_DEREF" => "dict_or_closure_read",
        _ => return None,
    })
}
fn instruction_names(ins: &Value) -> Vec<&str> {
    if ins["argval"]["type"] == "tuple" {
        ins["argval"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["value"].as_str().unwrap())
            .collect()
    } else {
        vec![ins["argval"]["value"].as_str().unwrap()]
    }
}
#[test]
fn pyc_final_versions_trace_every_explicit_name_and_packed_target_to_fetch() {
    for &(version, _) in VERSIONS {
        for label in ["graph", "semantics", "extended"] {
            let path = corpus(version, &format!("{label}.timestamp.pyc"));
            let expected = oracle(version, label);
            let mut names = std::collections::BTreeSet::new();
            for code in expected["codes"].as_array().unwrap() {
                for ins in code["dis_instructions"].as_array().unwrap() {
                    if name_operation(ins["opname"].as_str().unwrap(), 0).is_some() {
                        names.extend(instruction_names(ins));
                    }
                }
            }
            // Extended prefixes are covered once at their genuine >255 operand.
            if label == "extended" {
                names.retain(|name| *name == "local_299");
            }
            let mut fetched = std::collections::BTreeMap::<usize, Value>::new();
            for name in names {
                let all = rows(&command("xref", &path, &["--name", name]));
                assert_eq!(all[0]["query"]["name"], name);
                assert!(all[0]["query"]["at"].is_null());
                assert_eq!(all.last().unwrap()["complete"], true);
                let hits: Vec<_> = all.iter().filter(|r| r["record"] == "reference").collect();
                let mut wanted = Vec::new();
                for (owner, code) in expected["codes"].as_array().unwrap().iter().enumerate() {
                    for ins in code["dis_instructions"].as_array().unwrap() {
                        let opname = ins["opname"].as_str().unwrap();
                        if name_operation(opname, 0).is_none() {
                            continue;
                        }
                        for (position, value) in instruction_names(ins).iter().enumerate() {
                            if *value == name {
                                wanted.push((
                                    owner,
                                    ins,
                                    position,
                                    name_operation(opname, position).unwrap(),
                                ));
                            }
                        }
                    }
                }
                assert_eq!(hits.len(), wanted.len(), "{version}/{label}/{name}");
                for (hit, (owner, ins, position, operation)) in hits.into_iter().zip(wanted) {
                    assert_eq!(hit["detail"]["code_index"], owner);
                    assert_eq!(hit["detail"]["offset"], ins["offset"]);
                    assert_eq!(hit["detail"]["operand"], ins["arg"]);
                    assert_eq!(hit["detail"]["raw_operand"], ins["raw_arg_byte"]);
                    assert_eq!(hit["detail"]["operation"], operation);
                    let fetched = fetched
                        .entry(owner)
                        .or_insert_with(|| view(&path, hit["context_at"].as_str().unwrap()));
                    assert_eq!(fetched["at"], hit["context_at"]);
                    let unit = fetched["detail"]["instructions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|i| i["offset"] == ins["offset"])
                        .unwrap();
                    assert_eq!(hit["detail"]["target"], unit["targets"][position]);
                    assert_eq!(hit["start"], unit["start"]);
                    assert_eq!(hit["end"], unit["end"]);
                    let target = &hit["detail"]["target"];
                    let value = fetched["detail"]["values"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|v| v["id"] == target["value_id"])
                        .unwrap();
                    assert_eq!(value["value"], name);
                }
            }
        }
    }
}

fn literal_multiset(value: &Value, output: &mut Vec<(String, String)>) {
    match value["type"].as_str().unwrap() {
        "str" => output.push((
            "str".into(),
            value["utf8_surrogatepass_hex"].as_str().unwrap().into(),
        )),
        "bytes" => output.push(("bytes".into(), value["hex"].as_str().unwrap().into())),
        "tuple" | "frozenset" => {
            for child in value["items"].as_array().unwrap() {
                literal_multiset(child, output);
            }
        }
        _ => {}
    }
}
#[test]
fn pyc_final_versions_preserve_literal_occurrences_and_complete_search_refs_fetch_in_all_modes() {
    for &(version, minor) in VERSIONS {
        let expected = oracle(version, "graph");
        for mode in [
            "timestamp.pyc",
            "hash-unchecked.pyc",
            "hash-checked.pyc",
            "marshal",
        ] {
            let path = corpus(version, &format!("graph.{mode}"));
            let bytes = fs::read(&path).unwrap();
            let base_args = if mode == "marshal" {
                vec!["--version", minor]
            } else {
                Vec::new()
            };
            let mut name_args = base_args.clone();
            name_args.extend(["--name", "cell"]);
            let named = rows(&command("xref", &path, &name_args));
            assert_eq!(named.last().unwrap()["complete"], true);
            assert!(named.last().unwrap()["records"].as_u64().unwrap() > 0);
            let hit = &named[1];
            let owner = hit["detail"]["code_index"].as_u64().unwrap() as usize;
            let mut fetch_args = vec!["--at", hit["context_at"].as_str().unwrap()];
            fetch_args.extend(&base_args);
            let output = command("get-code", &path, &fetch_args);
            success(&output);
            let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                fetched["detail"]["bytecode_hex"],
                expected["codes"][owner]["code_hex"]
            );
            let unit = fetched["detail"]["instructions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|unit| unit["offset"] == hit["detail"]["offset"])
                .unwrap();
            assert_eq!(unit["start"], hit["start"]);
            assert_eq!(unit["targets"][0], hit["detail"]["target"]);
            let all = rows(&command("strings", &path, &base_args));
            assert_eq!(all.last().unwrap()["complete"], true);
            let literals: Vec<_> = all.iter().filter(|r| r["record"] == "string").collect();
            let mut identities = std::collections::HashSet::new();
            for (owner, code) in expected["codes"].as_array().unwrap().iter().enumerate() {
                for (slot, constant) in code["consts"].as_array().unwrap().iter().enumerate() {
                    let mut wanted = Vec::new();
                    literal_multiset(constant, &mut wanted);
                    wanted.sort();
                    let mut observed: Vec<_> = literals
                        .iter()
                        .filter(|l| {
                            l["detail"]["code_index"] == owner
                                && l["detail"]["constant_slot"] == slot
                        })
                        .map(|l| {
                            (
                                l["detail"]["kind"].as_str().unwrap().to_owned(),
                                l["detail"]["raw_bytes"].as_str().unwrap().to_owned(),
                            )
                        })
                        .collect();
                    observed.sort();
                    assert_eq!(observed, wanted, "{version}/{mode}/{owner}/{slot}");
                }
            }
            for literal in &literals {
                assert!(identities.insert(literal["at"].as_str().unwrap()));
                let d = &literal["detail"];
                let start = d["payload_start"].as_u64().unwrap() as usize;
                let end = d["payload_end"].as_u64().unwrap() as usize;
                let raw = bytes[start..end]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                assert_eq!(d["raw_bytes"], raw);
                if d["kind"] == "bytes" || raw == "eda080" {
                    assert!(d["value"].is_null());
                } else {
                    assert_eq!(d["value"], std::str::from_utf8(&bytes[start..end]).unwrap());
                }
                let path_string = d["constant_path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i.as_u64().unwrap().to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                assert_eq!(
                    literal["at"],
                    format!("const:{}:{path_string}", d["code_index"])
                );
                // Every timestamp occurrence exercises XREF, including aliases,
                // containing tuples and an unreferenced singleton frozenset. Other
                // modes replay a nonzero workflow with their own returned IDs.
                if mode != "timestamp.pyc" && d["value"] != "closure-marker" {
                    continue;
                }
                let mut args = base_args.clone();
                args.extend(["--at", literal["at"].as_str().unwrap()]);
                let refs = rows(&command("xref", &path, &args));
                assert_eq!(refs[0]["query"]["at"], literal["at"]);
                assert_eq!(refs.last().unwrap()["complete"], true);
                let owner = d["code_index"].as_u64().unwrap() as usize;
                let wanted: Vec<_> = expected["codes"][owner]["dis_instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|i| {
                        matches!(
                            i["opname"].as_str(),
                            Some("LOAD_CONST" | "RETURN_CONST" | "KW_NAMES")
                        ) && i["arg"] == d["constant_slot"]
                    })
                    .collect();
                let hits: Vec<_> = refs.iter().filter(|r| r["record"] == "reference").collect();
                assert_eq!(hits.len(), wanted.len());
                for (hit, ins) in hits.iter().zip(wanted) {
                    assert_eq!(hit["context_at"], literal["context_at"]);
                    assert_eq!(hit["detail"]["offset"], ins["offset"]);
                    assert_eq!(hit["detail"]["constant_path"], d["constant_path"]);
                    let mut args = vec!["--at", hit["context_at"].as_str().unwrap()];
                    args.extend(&base_args);
                    let output = command("get-code", &path, &args);
                    success(&output);
                    let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
                    assert_eq!(
                        fetched["detail"]["bytecode_hex"],
                        expected["codes"][owner]["code_hex"]
                    );
                    let unit = fetched["detail"]["instructions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|u| u["offset"] == ins["offset"])
                        .unwrap();
                    assert_eq!(unit["targets"][0], hit["detail"]["target"]);
                    assert_eq!(unit["start"], hit["start"]);
                    assert_eq!(unit["end"], hit["end"]);
                }
            }
        }
    }
}

fn bytes_field(out: &mut Vec<u8>, data: &[u8]) {
    out.push(b's');
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
}
// Worked original marshal fields, independent of the backend parser.
fn raw_code(
    minor: u8,
    body: &[u8],
    consts: &[u8],
    names: &[u8],
    lines: &[u8],
    exceptions: &[u8],
) -> Vec<u8> {
    let mut out = vec![b'c'];
    let fields: Vec<u32> = if minor <= 10 {
        vec![0, 0, 0, 0, 2, 0]
    } else {
        vec![0, 0, 0, 2, 0]
    };
    for value in fields {
        out.extend(value.to_le_bytes());
    }
    bytes_field(&mut out, body);
    out.extend(consts);
    out.extend(names);
    out.extend(b")\0");
    if minor <= 10 {
        out.extend(b")\0)\0");
    } else {
        bytes_field(&mut out, b"");
    }
    out.extend(b"z\x01xz\x01x");
    if minor >= 11 {
        out.extend(b"z\x01x");
    }
    out.extend(1_u32.to_le_bytes());
    bytes_field(&mut out, lines);
    if minor >= 11 {
        bytes_field(&mut out, exceptions);
    }
    out
}
fn opcode_metadata(version: &str) -> Value {
    let data: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/pyc/matrix/interpreters.json"),
        )
        .unwrap(),
    )
    .unwrap();
    data.as_array()
        .unwrap()
        .iter()
        .find(|m| {
            m["version_info"].as_array().unwrap()[..3]
                .iter()
                .map(|v| v.as_u64().unwrap().to_string())
                .collect::<Vec<_>>()
                .join(".")
                == version
        })
        .unwrap()
        .clone()
}
#[test]
fn pyc_final_versions_use_original_frozenset_child_order_for_occurrence_ids() {
    for &(version, minor) in VERSIONS {
        let meta = opcode_metadata(version);
        let number = meta["version_info"][1].as_u64().unwrap() as u8;
        let load = meta["opmap"]["LOAD_CONST"].as_u64().unwrap() as u8;
        let ret = meta["opmap"]["RETURN_VALUE"].as_u64().unwrap() as u8;
        let data = raw_code(
            number,
            &[load, 0, ret, 0],
            b")\x01>\x02\0\0\0z\x01zz\x01a",
            b")\0",
            &[],
            &[],
        );
        let (_dir, path) = fixture(&data, "frozenset.marshal");
        let all = rows(&command("strings", &path, &["--version", minor]));
        assert_eq!(all[1]["detail"]["value"], "z");
        assert_eq!(all[1]["detail"]["constant_path"], serde_json::json!([0, 0]));
        assert_eq!(all[2]["detail"]["value"], "a");
        assert_eq!(all[2]["detail"]["constant_path"], serde_json::json!([0, 1]));
        for literal in &all[1..3] {
            let refs = rows(&command(
                "xref",
                &path,
                &["--version", minor, "--at", literal["at"].as_str().unwrap()],
            ));
            assert_eq!(refs.last().unwrap()["records"], 1);
            assert_eq!(refs[1]["detail"]["relation"], "containing_constant");
            assert_eq!(
                refs[1]["detail"]["constant_path"],
                literal["detail"]["constant_path"]
            );
            success(&command(
                "get-code",
                &path,
                &[
                    "--at",
                    refs[1]["context_at"].as_str().unwrap(),
                    "--version",
                    minor,
                ],
            ));
        }
    }
}
#[test]
fn pyc_final_versions_distinguish_malformed_selected_evidence_from_unknown_opcodes_atomically() {
    for &(version, minor) in VERSIONS {
        let meta = opcode_metadata(version);
        let number = meta["version_info"][1].as_u64().unwrap() as u8;
        let op = |name: &str| meta["opmap"][name].as_u64().unwrap() as u8;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raw.marshal");
        let dest = dir.path().join("keep.json");
        let mut cases = vec![
            (
                vec![op("NOP")],
                vec![],
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
            (
                vec![op("EXTENDED_ARG"), 1],
                vec![],
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
            (
                vec![op("EXTENDED_ARG"), 1, op("NOP"), 0],
                vec![],
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
            (
                vec![op("LOAD_CONST"), 255],
                vec![],
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
            (
                vec![op("LOAD_FAST"), 255],
                vec![],
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
            (
                vec![op("JUMP_FORWARD"), 255],
                vec![],
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
            (
                vec![op("NOP"), 0],
                vec![0x80],
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
            (
                vec![op("NOP"), 0],
                if number <= 10 {
                    vec![4, 0]
                } else {
                    vec![0x87, 0]
                },
                vec![],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ),
        ];
        let mut wide = [op("EXTENDED_ARG"), 1].repeat(4);
        wide.extend([op("LOAD_CONST"), 0]);
        cases.push((wide, vec![], vec![], b")\x01N".to_vec(), b")\0".to_vec()));
        // names must remain original string objects, even for a zero-hit query.
        let mut global = vec![op("LOAD_GLOBAL"), 0];
        // Known final cache counts are independently pinned in the metadata list.
        let cache_count = if number == 11 {
            5
        } else if number >= 12 {
            4
        } else {
            0
        };
        global.extend(vec![0; cache_count * 2]);
        cases.push((
            global,
            vec![],
            vec![],
            b")\x01N".to_vec(),
            b")\x01i\x01\0\0\0".to_vec(),
        ));
        if number >= 11 {
            cases.push((
                vec![op("LOAD_GLOBAL"), 0],
                vec![],
                vec![],
                b")\x01N".to_vec(),
                b")\x01z\x01g".to_vec(),
            ));
            cases.push((
                vec![op("NOP"), 0],
                vec![],
                vec![0x80],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ));
            cases.push((
                vec![op("NOP"), 0],
                vec![],
                vec![0x80, 1, 1, 0],
                b")\x01N".to_vec(),
                b")\0".to_vec(),
            ));
        }
        if number == 13 {
            cases.push((
                vec![op("EXTENDED_ARG"), 1, op("WITH_EXCEPT_START"), 0],
                vec![],
                vec![],
                b")\0".to_vec(),
                b")\0".to_vec(),
            ));
        }
        for (body, lines, exceptions, consts, names) in cases {
            let data = raw_code(number, &body, &consts, &names, &lines, &exceptions);
            fs::write(&path, &data).unwrap();
            fs::write(&dest, b"keep").unwrap();
            let codes = rows(&command("codes", &path, &["--version", minor]));
            failure(
                command(
                    "get-code",
                    &path,
                    &[
                        "--at",
                        codes[1]["at"].as_str().unwrap(),
                        "--version",
                        minor,
                        "-o",
                        dest.to_str().unwrap(),
                    ],
                ),
                1,
                "INVALID_INPUT",
            );
            // References validate instruction operands/jumps, while location and
            // exception decoding belongs to FETCH.
            if lines.is_empty() && exceptions.is_empty() {
                failure(
                    command(
                        "xref",
                        &path,
                        &[
                            "--name",
                            "no-hit",
                            "--version",
                            minor,
                            "-o",
                            dest.to_str().unwrap(),
                        ],
                    ),
                    1,
                    "INVALID_INPUT",
                );
            }
            assert_eq!(fs::read(&dest).unwrap(), b"keep");
            assert_eq!(fs::read(&path).unwrap(), data);
        }
        let unknown = (0..=255)
            .find(|n| {
                !meta["opmap"]
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|v| v.as_u64() == Some(*n))
            })
            .unwrap() as u8;
        let data = raw_code(number, &[unknown, 255], b")\0", b")\0", &[], &[]);
        fs::write(&path, &data).unwrap();
        let codes = rows(&command("codes", &path, &["--version", minor]));
        let output = command(
            "get-code",
            &path,
            &["--at", codes[1]["at"].as_str().unwrap(), "--version", minor],
        );
        success(&output);
        let fetched: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            fetched["detail"]["instructions"][0]["status"],
            "UNSUPPORTED_OPCODE"
        );
        assert_eq!(fetched["detail"]["complete"], true);
        let refs = rows(&command(
            "xref",
            &path,
            &["--name", "no-hit", "--version", minor],
        ));
        assert_eq!(refs.last().unwrap()["complete"], false);
        assert_eq!(refs.last().unwrap()["detail"]["unsupported_count"], 1);
    }
}

#[test]
fn pyc_final_versions_reject_unverified_headers_versions_and_stale_ids() {
    for &(version, minor) in VERSIONS {
        let original = corpus(version, "graph.timestamp.pyc");
        let data = fs::read(&original).unwrap();
        let codes = code_rows(version, "graph");
        let id = codes[0]["at"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.pyc");
        let dest = dir.path().join("keep.json");
        let mut variants = Vec::new();
        for magic in [0_u16, 3412, 3424, 3438, 3494, 3530, 3570, 160, 21150, 12897] {
            let mut changed = data.clone();
            changed[..2].copy_from_slice(&magic.to_le_bytes());
            variants.push(changed);
        }
        for flags in [2_u32, 4, 7, u32::MAX] {
            let mut changed = data.clone();
            changed[4..8].copy_from_slice(&flags.to_le_bytes());
            variants.push(changed);
        }
        let mut separator = data.clone();
        separator[2] = 0;
        variants.push(separator);
        for size in [0, 3, 4, 7, 15, 16, data.len() - 1] {
            variants.push(data[..size].to_vec());
        }
        let mut trailing = data.clone();
        trailing.push(0);
        variants.push(trailing);
        for variant in variants {
            fs::write(&path, &variant).unwrap();
            fs::write(&dest, b"keep").unwrap();
            for (cmd, args) in [
                ("codes", vec![]),
                ("strings", vec![]),
                ("xref", vec!["--name", "none"]),
                ("get-code", vec!["--at", id]),
            ] {
                let mut args = args;
                args.extend(["-o", dest.to_str().unwrap()]);
                failure(command(cmd, &path, &args), 1, "INVALID_INPUT");
                assert_eq!(fs::read(&dest).unwrap(), b"keep");
            }
            assert_eq!(fs::read(&path).unwrap(), variant);
        }
        for unsupported in ["2.7", "3.7", "3.14"] {
            failure(
                command(
                    "codes",
                    &corpus(version, "graph.marshal"),
                    &["--version", unsupported],
                ),
                1,
                "INVALID_INPUT",
            );
        }
        let mut changed = data.clone();
        changed[8] ^= 1;
        fs::write(&path, &changed).unwrap();
        // A coordinate names a code object in the input being queried; the header carries
        // identity, so a rewritten file still resolves the same index.
        success(&command("get-code", &path, &["--at", id]));
        let stale = id.to_owned();
        failure(
            command("get-code", &path, &["--at", &format!("{stale}:extra")]),
            2,
            "INVALID_QUERY",
        );
        failure(
            command(
                "xref",
                &path,
                &["--name", "arg", "--within", &format!("{stale}:extra")],
            ),
            1,
            "SOURCE_MISMATCH",
        );
        let zero = rows(&command(
            "xref",
            &original,
            &["--name", "no-exact-name-matches"],
        ));
        assert_eq!(zero.last().unwrap()["records"], 0);
        assert_eq!(zero.last().unwrap()["complete"], true);
        let zero = rows(&command(
            "strings",
            &original,
            &["--filter", "no-literal-matches"],
        ));
        assert_eq!(zero.last().unwrap()["records"], 0);
        assert_eq!(zero.last().unwrap()["complete"], true);
        let raw = rows(&command(
            "codes",
            &corpus(version, "graph.marshal"),
            &["--version", minor],
        ));
        assert_eq!(raw[0]["query"]["version"], minor);
    }
}
fn analysis_perf(output: &Output) -> Value {
    output
        .stderr
        .split(|b| *b == b'\n')
        .filter_map(|s| serde_json::from_slice::<Value>(s).ok())
        .find(|v| v["scope"] == "analysis")
        .unwrap()
}
#[test]
fn pyc_final_versions_enforce_every_query_budget_at_its_actual_boundary_atomically() {
    let budget = rxd_core::Budget::default();
    let probe = corpus(VERSIONS[0].0, "graph.timestamp.pyc");
    for cmd in ["codes", "strings", "xref", "get-code"] {
        assert_no_budget_flags(&probe, cmd);
    }
    for &(version, _) in VERSIONS {
        let path = corpus(version, "graph.timestamp.pyc");
        let data = fs::read(&path).unwrap();
        let codes = code_rows(version, "graph");
        let at = codes[0]["at"].as_str().unwrap();
        let units = codes[0]["detail"]["bytecode_bytes"].as_u64().unwrap() as usize / 2;
        let max_units = codes
            .iter()
            .map(|c| c["detail"]["bytecode_bytes"].as_u64().unwrap() as usize / 2)
            .max()
            .unwrap();
        let mut base = shared_static_limits();
        base.input_bytes = data.len();
        let work_of = |perf: &Value| perf["counters"]["work"].as_u64().unwrap() as usize;

        // codes
        let (perf, selected) = codes_library(&path, None, None, None, None, base);
        assert!(selected.is_ok(), "{version}/codes");
        let work = work_of(&perf_value(&perf));
        let mut less = base;
        less.work = work - 1;
        assert_eq!(
            codes_library(&path, None, None, None, None, less)
                .1
                .err()
                .unwrap()
                .code,
            "RESOURCE_LIMIT",
            "{version}/codes"
        );

        // strings
        let (perf, literals) = strings_library(&path, None, None, None, base);
        assert!(literals.is_ok(), "{version}/strings");
        let work = work_of(&perf_value(&perf));
        let mut less = base;
        less.work = work - 1;
        assert_eq!(
            strings_library(&path, None, None, None, less)
                .1
                .err()
                .unwrap()
                .code,
            "RESOURCE_LIMIT",
            "{version}/strings"
        );

        // xref, including the per-code instruction ceiling
        let (perf, scan) = xref_name_library(&path, None, "arg", None, base, max_units);
        assert!(scan.is_ok(), "{version}/xref");
        let work = work_of(&perf_value(&perf));
        let mut less = base;
        less.work = work - 1;
        assert_eq!(
            xref_name_library(&path, None, "arg", None, less, max_units)
                .1
                .err()
                .unwrap()
                .code,
            "RESOURCE_LIMIT",
            "{version}/xref"
        );
        assert_eq!(
            xref_name_library(&path, None, "arg", None, base, max_units - 1)
                .1
                .err()
                .unwrap()
                .code,
            "RESOURCE_LIMIT",
            "{version}/xref instructions"
        );

        // get-code
        let (perf, evidence) = get_code_library(&path, None, at, base, units);
        assert!(evidence.is_ok(), "{version}/get-code");
        let work = work_of(&perf_value(&perf));
        let mut less = base;
        less.work = work - 1;
        assert_eq!(
            get_code_library(&path, None, at, less, units)
                .1
                .err()
                .unwrap()
                .code,
            "RESOURCE_LIMIT",
            "{version}/get-code"
        );
        assert_eq!(
            get_code_library(&path, None, at, base, units - 1)
                .1
                .err()
                .unwrap()
                .code,
            "RESOURCE_LIMIT",
            "{version}/get-code instructions"
        );

        // The CLI still projects the one shared table for every command.
        for (cmd, args) in [
            ("codes", vec![]),
            ("strings", vec![]),
            ("xref", vec!["--name", "arg"]),
            ("get-code", vec!["--at", at]),
        ] {
            let mut instrumented = args;
            instrumented.push("--perf");
            let output = command(cmd, &path, &instrumented);
            success(&output);
            let perf = analysis_perf(&output);
            assert_eq!(
                perf["resource_limits"]["work"],
                Value::from(budget.work),
                "{version}/{cmd}"
            );
        }

        // A rejected selector leaves an existing output file untouched.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("keep.json");
        fs::write(&dest, b"keep").unwrap();
        failure(
            command(
                "get-code",
                &path,
                &["--at", "code:99999", "-o", dest.to_str().unwrap()],
            ),
            2,
            "INVALID_QUERY",
        );
        assert_eq!(fs::read(&dest).unwrap(), b"keep");
    }
}

#[test]
fn pyc_old_jump_units_and_pep626_zero_length_no_line_intervals_are_exact() {
    for &(version, minor) in &VERSIONS[..3] {
        let meta = opcode_metadata(version);
        let number = meta["version_info"][1].as_u64().unwrap() as u8;
        let jump = meta["opmap"]["JUMP_ABSOLUTE"].as_u64().unwrap() as u8;
        let nop = meta["opmap"]["NOP"].as_u64().unwrap() as u8;
        let valid = if number == 10 { 1 } else { 2 };
        let bad = if number == 10 { 2 } else { 1 };
        for (arg, accepted) in [(valid, true), (bad, false)] {
            let (_dir, path) = fixture(
                raw_code(number, &[jump, arg, nop, 0], b")\0", b")\0", &[], &[]),
                "jump.marshal",
            );
            let codes = rows(&command("codes", &path, &["--version", minor]));
            let output = command(
                "get-code",
                &path,
                &["--at", codes[1]["at"].as_str().unwrap(), "--version", minor],
            );
            if accepted {
                success(&output);
                let v: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(v["detail"]["instructions"][0]["jump_target"], 2);
            } else {
                failure(output, 1, "INVALID_INPUT");
            }
        }
    }
    let (_dir, path) = fixture(
        raw_code(
            10,
            &[9, 0, 9, 0, 9, 0],
            b")\0",
            b")\0",
            &[0, 127, 2, 1, 2, 128, 2, 255],
            &[],
        ),
        "line.marshal",
    );
    let codes = rows(&command("codes", &path, &["--version", "3.10"]));
    let output = command(
        "get-code",
        &path,
        &[
            "--at",
            codes[1]["at"].as_str().unwrap(),
            "--version",
            "3.10",
        ],
    );
    success(&output);
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mapping: Vec<_> = v["detail"]["line_mapping"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| serde_json::json!([l["start"], l["end"], l["line"], l["column"], l["end_line"]]))
        .collect();
    assert_eq!(
        mapping,
        vec![
            serde_json::json!([0, 2, 129, null, null]),
            serde_json::json!([2, 4, null, null, null]),
            serde_json::json!([4, 6, 128, null, null])
        ]
    );
}

#[test]
fn pyc_final_versions_navigate_pinned_actual_email_policy_modules_offline() {
    for &(version, _) in VERSIONS {
        let label = "email-policy";
        let path = corpus(version, &format!("{label}.timestamp.pyc"));
        let expected = oracle(version, label);
        let codes = code_rows(version, label);
        assert_eq!(codes.len(), expected["codes"].as_array().unwrap().len());
        for (row, code) in codes.iter().zip(expected["codes"].as_array().unwrap()) {
            assert_eq!(row["detail"]["name"], code["name"]);
            assert_eq!(
                row["detail"]["bytecode_bytes"],
                code["code_hex"].as_str().unwrap().len() / 2
            );
        }
        let refs = rows(&command("xref", &path, &["--name", "value"]));
        assert!(refs.last().unwrap()["records"].as_u64().unwrap() > 0);
        assert_eq!(refs.last().unwrap()["complete"], true);
        let hit = &refs[1];
        let owner = hit["detail"]["code_index"].as_u64().unwrap() as usize;
        let fetched = view(&path, hit["context_at"].as_str().unwrap());
        assert_eq!(
            fetched["detail"]["bytecode_hex"],
            expected["codes"][owner]["code_hex"]
        );
        let strings = rows(&command("strings", &path, &["--filter", "header"]));
        assert!(strings.last().unwrap()["records"].as_u64().unwrap() > 0);
        for literal in strings.iter().filter(|r| r["record"] == "string") {
            let refs = rows(&command(
                "xref",
                &path,
                &["--at", literal["at"].as_str().unwrap()],
            ));
            assert_eq!(refs.last().unwrap()["complete"], true);
            for hit in refs.iter().filter(|r| r["record"] == "reference") {
                assert_eq!(hit["context_at"], literal["context_at"]);
                success(&command(
                    "get-code",
                    &path,
                    &["--at", hit["context_at"].as_str().unwrap()],
                ));
            }
        }
    }
}
