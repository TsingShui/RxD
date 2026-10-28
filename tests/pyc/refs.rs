use super::*;

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/pyc/3.12.14")
        .join(name)
}
fn refs(path: &Path, name: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["pyc", "xref"])
        .arg(path)
        .args(["--kind", "name", "--at", name])
        .args(args)
        .output()
        .unwrap()
}
fn rows(output: &Output) -> Vec<Value> {
    success(output);
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
#[test]
fn pyc_refs_distinguish_all_explicit_name_operations_with_independent_operands() {
    let path = corpus("refs.timestamp.pyc");
    let oracle: Value =
        serde_json::from_slice(&fs::read(corpus("refs.oracle.json")).unwrap()).unwrap();
    let operations = [
        ("LOAD_NAME", "name_read"),
        ("STORE_NAME", "name_write"),
        ("DELETE_NAME", "name_delete"),
        ("LOAD_GLOBAL", "global_read"),
        ("STORE_GLOBAL", "global_write"),
        ("DELETE_GLOBAL", "global_delete"),
        ("LOAD_FAST", "local_read"),
        ("LOAD_FAST_CHECK", "local_read"),
        ("STORE_FAST", "local_write"),
        ("DELETE_FAST", "local_delete"),
        ("LOAD_FAST_AND_CLEAR", "local_read_clear"),
        ("MAKE_CELL", "cell_create"),
        ("LOAD_CLOSURE", "closure_capture"),
        ("LOAD_DEREF", "closure_read"),
        ("STORE_DEREF", "closure_write"),
        ("DELETE_DEREF", "closure_delete"),
        ("LOAD_ATTR", "attribute_read"),
        ("LOAD_SUPER_ATTR", "attribute_read"),
        ("STORE_ATTR", "attribute_write"),
        ("DELETE_ATTR", "attribute_delete"),
        ("IMPORT_NAME", "import_name"),
        ("IMPORT_FROM", "import_from"),
        ("LOAD_FROM_DICT_OR_GLOBALS", "dict_or_global_read"),
        ("LOAD_FROM_DICT_OR_DEREF", "dict_or_closure_read"),
    ];
    let codes = oracle["codes"].as_array().unwrap();
    let mut names = std::collections::BTreeSet::new();
    let mut covered = std::collections::BTreeSet::new();
    for code in codes {
        for ins in code["dis_instructions"].as_array().unwrap() {
            if operations.iter().any(|(op, _)| ins["opname"] == *op) {
                names.insert(ins["argval"]["value"].as_str().unwrap());
                covered.insert(ins["opname"].as_str().unwrap());
            }
        }
    }
    assert_eq!(covered.len(), operations.len()); // Fixture/compiler really exercises every family.
    for name in names {
        let stream = rows(&refs(&path, name, &[]));
        let hits: Vec<_> = stream
            .iter()
            .filter(|r| r["record"] == "reference")
            .collect();
        let expected: Vec<_> = codes
            .iter()
            .enumerate()
            .flat_map(|(index, code)| {
                code["dis_instructions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|ins| {
                        operations.iter().any(|(op, _)| ins["opname"] == *op)
                            && ins["argval"]["value"] == name
                    })
                    .map(move |ins| (index, ins))
            })
            .collect();
        assert_eq!(hits.len(), expected.len(), "name={name}");
        for (hit, (index, ins)) in hits.into_iter().zip(expected) {
            assert_eq!(hit["detail"]["code_index"], index);
            assert_eq!(hit["detail"]["offset"], ins["offset"]);
            assert_eq!(hit["detail"]["name"], ins["opname"]);
            let operation = operations
                .iter()
                .find(|(op, _)| ins["opname"] == *op)
                .unwrap()
                .1;
            assert_eq!(hit["detail"]["operation"], operation);
            assert_eq!(hit["detail"]["operand"], ins["arg"]);
            let operand = ins["arg"].as_u64().unwrap() as usize;
            let opname = ins["opname"].as_str().unwrap();
            let local = opname.contains("FAST")
                || opname.contains("DEREF")
                || matches!(opname, "MAKE_CELL" | "LOAD_CLOSURE");
            let table = if local { "localsplus" } else { "names" };
            let slot = match opname {
                "LOAD_GLOBAL" | "LOAD_ATTR" => operand >> 1,
                "LOAD_SUPER_ATTR" => operand >> 2,
                _ => operand,
            };
            assert_eq!(hit["detail"]["target"]["table"], table);
            assert_eq!(hit["detail"]["target"]["index"], slot);
            let table_names = if local {
                &codes[index]["localsplus_names_by_operand"]
            } else {
                &codes[index]["names"]
            };
            assert_eq!(table_names[slot], name);
        }
    }
    // Genuine compiler EXTENDED_ARG resolves a local above the one-byte range.
    let stream = rows(&refs(&corpus("extended.timestamp.pyc"), "local_299", &[]));
    let hits: Vec<_> = stream
        .iter()
        .filter(|r| r["record"] == "reference")
        .collect();
    assert_eq!(hits.len(), 2);
    for hit in hits {
        assert_eq!(hit["detail"]["raw_operand"], 43);
        assert_eq!(hit["detail"]["operand"], 299);
        assert_eq!(hit["detail"]["target"]["index"], 299);
    }
}
