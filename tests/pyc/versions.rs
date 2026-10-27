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
fn view(path: &Path, at: &str) -> Value {
    let output = command("get-code", path, &["--at", at]);
    success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
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
                let all = rows(&command("xref", &path, &["--kind", "name", "--at", name]));
                assert!(all[0]["query"]["match"].is_null());
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

// Worked original marshal fields, independent of the backend parser.
