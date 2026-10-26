use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["html", "entries"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
fn extract(path: &Path, id: &str, destination: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["html", "extract"])
        .arg(path)
        .args(["--at", id, "-o"])
        .arg(destination)
        .args(extra)
        .output()
        .unwrap()
}
fn receipt(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout.iter().filter(|&&byte| byte == b'\n').count(),
        1
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn rows(output: &Output) -> Vec<Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    output
        .stdout
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect()
}
fn fails(output: Output, code: &str) {
    // Query-class codes and clap usage errors exit 2; input/resource failures exit 1.
    let expected = match code {
        "unexpected argument" | "INVALID_QUERY" | "NOT_FOUND" | "AMBIGUOUS" => 2,
        _ => 1,
    };
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(code));
}

#[test]
fn html_list_filters_exact_kinds_tags_and_decoded_duplicate_attributes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("filters.html");
    fs::write(&path, b"<DIV HREF='/a&amp;b' href='X=Y>Z' disabled><span>inside</span></DIV><div href='/a&b'>outside</div>").unwrap();
    let selected = rows(&run(
        &path,
        &["--kind", "element", "--tag", "DiV", "--attr", "HrEf=X=Y>Z"],
    ));
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0]["detail"]["tag"], "div");
    assert_eq!(selected[0]["detail"]["attrs"][0]["name"], "href");
    let decoded = rows(&run(&path, &["--attr", "href=/a&b"]));
    assert_eq!(decoded.len(), 2);
    assert!(
        decoded
            .iter()
            .any(|row| row["detail"]["attrs"][0]["value"] == "/a&b")
    );
    assert!(rows(&run(&path, &["--attr", "href=/a&amp;b"])).is_empty());
    assert_eq!(rows(&run(&path, &["--attr", "disabled"])).len(), 1);
    assert!(rows(&run(&path, &["--kind", "text", "--tag", "div"])).is_empty());
    assert!(rows(&run(&path, &["--attr", "href=x=y"])).is_empty());
    let empty_value = rows(&run(&path, &["--attr", "disabled="]));
    assert_eq!(empty_value.len(), 1);
    assert_eq!(empty_value[0]["detail"]["tag"], "div");
}

#[test]
fn html_list_within_includes_root_and_uses_relative_depth_without_rewriting_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.html");
    fs::write(
        &path,
        b"<main><section><p>A</p><!--C--></section><aside>B</aside></main>",
    )
    .unwrap();
    let all = rows(&run(&path, &[]));
    let nodes = &all[..];
    let section = nodes
        .iter()
        .find(|r| r["detail"]["tag"] == "section")
        .unwrap();
    let section_id = section["at"].as_str().unwrap();
    let filtered = rows(&run(&path, &["--within", section_id, "--depth", "1"]));
    let expected: Vec<_> = nodes
        .iter()
        .filter(|r| {
            r["detail"]["tag"] == "section" || r["detail"]["tag"] == "p" || r["record"] == "comment"
        })
        .collect();
    assert_eq!(
        filtered,
        expected.iter().map(|r| (*r).clone()).collect::<Vec<_>>()
    );
    assert_eq!(filtered[0]["detail"]["depth"], 1);
    assert_eq!(filtered[0]["parent_at"], nodes[0]["at"]);
    let only_root = rows(&run(&path, &["--within", section_id, "--depth", "0"]));
    assert_eq!(only_root.len(), 1);
    assert_eq!(only_root[0], *section);
    let text = rows(&run(&path, &["--kind", "text", "--within", section_id]));
    assert_eq!(text.len(), 1);
    assert_eq!(text[0]["detail"]["value"], "A");
    assert_eq!(text[0]["detail"]["depth"], 3);
    assert_eq!(
        text[0]["parent_at"],
        nodes.iter().find(|r| r["detail"]["tag"] == "p").unwrap()["at"]
    );
    assert_eq!(text[0]["context_at"], text[0]["parent_at"]);
    let top = rows(&run(&path, &["--depth", "0"]));
    assert_eq!(top.len(), 1);
    assert_eq!(top[0], nodes[0]);
    // Budgets come from the shared table, so the only thing a caller can still do is
    // ask for a shape; a budget flag is refused as an unknown argument.
    for flag in ["--max-depth", "--max-records", "--max-output-bytes"] {
        fails(run(&path, &[flag, "1"]), "unexpected argument");
    }
}

#[test]
fn html_list_rejects_untrusted_within_ids_and_invalid_selectors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ids.html");
    fs::write(&path, b"<div><p>hi</p></div>").unwrap();
    let all = rows(&run(&path, &[]));
    let id = all[0]["at"].as_str().unwrap();
    let text_id = all.iter().find(|r| r["record"] == "text").unwrap()["at"]
        .as_str()
        .unwrap();
    for candidate in [
        "not-an-id".to_owned(),
        id.replace(":0:", ":00:"),
        id.replacen("element:", "unknown:", 1),
    ] {
        fails(run(&path, &["--within", &candidate]), "INVALID_QUERY");
    }
    // A row prints the copy-only short coordinate, so it is a valid selector.
    assert!(run(&path, &["--within", id]).status.success());
    let non_element = run(&path, &["--within", text_id]);
    assert_eq!(non_element.status.code(), Some(2));
    assert!(non_element.stdout.is_empty());
    assert!(String::from_utf8_lossy(&non_element.stderr).contains("INVALID_QUERY"));
    for args in [
        vec!["--kind", "invalid"],
        vec!["--attr", ""],
        vec!["--attr", "bad name=x"],
        vec!["--attr", "<bad"],
    ] {
        let output = run(&path, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn html_list_filtered_file_is_atomic_and_identical_to_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.html");
    let target = dir.path().join("catalog.jsonl");
    fs::write(
        &path,
        b"<a href='/x&amp;y'>first</a><a href='/z'>second</a>",
    )
    .unwrap();
    let args = ["--tag", "A", "--attr", "HREF=/x&y"];
    let expected = run(&path, &args);
    assert_eq!(rows(&expected).len(), 1);
    fs::write(&target, b"old").unwrap();
    let mut to_file = args.to_vec();
    to_file.extend(["-o", target.to_str().unwrap()]);
    assert!(rows_file_success(&run(&path, &to_file)));
    assert_eq!(fs::read(&target).unwrap(), expected.stdout);
    let mut budget_flag = args.to_vec();
    budget_flag.extend(["--max-output-bytes", "1"]);
    fails(run(&path, &budget_flag), "unexpected argument");
    assert_eq!(fs::read(&target).unwrap(), expected.stdout);
}

#[test]
fn html_list_reports_source_bound_lexical_catalog_and_atomic_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.html");
    let source = include_bytes!("../fixtures/html/catalog.html");
    fs::write(&path, source).unwrap();
    let output = run(&path, &[]);
    let entries = rows(&output);
    assert_eq!(entries.len(), 5);
    let kinds: Vec<_> = entries[..]
        .iter()
        .map(|r| r["record"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["doctype", "comment", "element", "element", "text"]);
    let spans: Vec<_> = entries[..]
        .iter()
        .map(|r| (r["start"].as_u64().unwrap(), r["end"].as_u64().unwrap()))
        .collect();
    assert_eq!(spans, [(0, 15), (15, 23), (23, 81), (69, 73), (73, 75)]);
    for (row, (start, end)) in entries.iter().zip(spans) {
        assert_eq!(
            row["at"],
            format!("{}:{start}:{end}", row["record"].as_str().unwrap())
        );
        assert!(row["source_id"].is_null());
    }
    assert_eq!(
        entries[0]["detail"],
        json!({"value":"<!DOCTYPE html>","depth":0})
    );
    assert_eq!(entries[1]["detail"], json!({"value":"<!--x-->","depth":0}));
    assert_eq!(
        entries[2]["detail"],
        json!({"tag":"div","attrs":[{"name":"href","value":"/a&b"},{"name":"href","value":"x"},{"name":"disabled","value":""},{"name":"empty","value":""}],"self_closing":false,"void":false,"depth":0,"end_tag":"explicit","content_start":69,"content_end":75})
    );
    assert_eq!(entries[2]["context_at"], entries[2]["at"]);
    assert_eq!(entries[3]["parent_at"], entries[2]["at"]);
    assert_eq!(entries[3]["detail"]["end_tag"], "not_applicable");
    assert_eq!(entries[4]["detail"], json!({"value":"Hi","depth":1}));
    assert_eq!(entries[4]["context_at"], entries[2]["at"]);
    assert_eq!(output.stdout, run(&path, &[]).stdout);
    let target = dir.path().join("catalog.jsonl");
    let written = run(&path, &["-o", target.to_str().unwrap()]);
    assert!(rows_file_success(&written));
    assert_eq!(fs::read(target).unwrap(), output.stdout);
}
#[test]
fn html_extract_fetches_every_catalog_kind_with_artifact_identity_and_container_origin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.html");
    let source = b"<!DOCTYPE html><!--note--><script data-long='&amp;12345678901234567890'>\r\nconst x = 1;\0\xe2\x80\xa8</script>";
    fs::write(&path, source).unwrap();
    let catalog = rows(&run(&path, &[]));
    let destination = dir.path().join("arbitrary-name");
    let hash = rxd_core::identity::input_id(source);
    for node in &catalog[1..catalog.len() - 1] {
        let id = node["at"].as_str().unwrap();
        let start = node["start"].as_u64().unwrap() as usize;
        let end = node["end"].as_u64().unwrap() as usize;
        let output = extract(&path, id, &destination, &[]);
        let record = receipt(&output);
        let selected = &source[start..end];
        assert_eq!(fs::read(&destination).unwrap(), selected);
        assert_eq!(
            record,
            json!({
                "record":"file", "backend":"html", "command":"extract", "schema_version":2,
                "at":null, "parent_at":null, "context_at":null,
                "source_id":rxd_core::identity::input_id(selected), "start":0, "end":selected.len(),
                "detail":{"complete":true,"path":destination.to_str().unwrap(),"file_bytes":selected.len(),
                    "representation":"original","part":"outer",
                    "origin":{"container":path.to_str().unwrap(),"format":"html","source_id":hash,
                        "node_id":id,"kind":node["record"],"part":"outer","start":start,"end":end}}
            })
        );
    }
    let element = catalog
        .iter()
        .find(|row| row["detail"]["tag"] == "script")
        .unwrap();
    let start = element["detail"]["content_start"].as_u64().unwrap() as usize;
    let end = element["detail"]["content_end"].as_u64().unwrap() as usize;
    let record = receipt(&extract(
        &path,
        element["at"].as_str().unwrap(),
        &destination,
        &["--content"],
    ));
    assert_eq!(fs::read(destination).unwrap(), source[start..end]);
    assert_eq!(record["detail"]["origin"]["start"], start);
    assert_eq!(record["detail"]["origin"]["end"], end);
    assert_eq!(record["detail"]["part"], "content");
}

#[test]
fn html_extract_rejects_content_on_nonelements_and_empty_content_without_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.html");
    fs::write(&path, b"<!DOCTYPE html><!--c--><div></div><span>x</span>").unwrap();
    let catalog = rows(&run(&path, &[]));
    let nodes = &catalog[..];
    let destination = dir.path().join("artifact");
    for kind in ["doctype", "comment", "text"] {
        let node = nodes.iter().find(|r| r["record"] == kind).unwrap();
        fs::write(&destination, b"old").unwrap();
        let output = extract(
            &path,
            node["at"].as_str().unwrap(),
            &destination,
            &["--content"],
        );
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_QUERY"));
        assert_eq!(fs::read(&destination).unwrap(), b"old");
    }
    let empty_div = nodes.iter().find(|r| r["detail"]["tag"] == "div").unwrap();
    fs::write(&destination, b"old").unwrap();
    fails(
        extract(
            &path,
            empty_div["at"].as_str().unwrap(),
            &destination,
            &["--content"],
        ),
        "NOT_FOUND",
    );
    assert_eq!(fs::read(&destination).unwrap(), b"old");
    let span = nodes.iter().find(|r| r["detail"]["tag"] == "span").unwrap();
    let record = receipt(&extract(
        &path,
        span["at"].as_str().unwrap(),
        &destination,
        &["--content"],
    ));
    assert_eq!(fs::read(&destination).unwrap(), b"x");
    assert_eq!(record["detail"]["part"], "content");
}

#[test]
fn html_extract_reports_uniform_id_taxonomy_and_refuses_input_alias() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.html");
    fs::write(&path, b"<div><p>hi</p></div>").unwrap();
    let catalog = rows(&run(&path, &[]));
    let id = catalog[0]["at"].as_str().unwrap();
    let destination = dir.path().join("artifact");
    fs::write(&destination, b"old").unwrap();
    for candidate in [
        "not-an-id".to_owned(),
        id.replace(":0:", ":00:"),
        id.replacen("element:", "unknown:", 1),
    ] {
        fails(
            extract(&path, &candidate, &destination, &[]),
            "INVALID_QUERY",
        );
        assert_eq!(fs::read(&destination).unwrap(), b"old");
    }
    for target in [path.clone(), dir.path().join(".").join("page.html")] {
        let output = extract(&path, id, &target, &[]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("INVALID_QUERY"));
        assert_eq!(fs::read(&path).unwrap(), b"<div><p>hi</p></div>");
    }
}

#[test]
fn html_extract_enforces_combined_artifact_and_receipt_budget_before_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.html");
    fs::write(&path, b"<script>const x = 1;</script>").unwrap();
    let catalog = rows(&run(&path, &[]));
    let element = catalog
        .iter()
        .find(|r| r["detail"]["tag"] == "script")
        .unwrap();
    let destination = dir.path().join("artifact.js");
    fs::write(&destination, b"old").unwrap();
    fails(
        extract(
            &path,
            element["at"].as_str().unwrap(),
            &destination,
            &["--max-output-bytes", "1"],
        ),
        "unexpected argument",
    );
    assert_eq!(fs::read(&destination).unwrap(), b"old");
    // `--content` still extracts the inner range; the byte budget is internal now.
    fs::write(&destination, b"old").unwrap();
    let content = extract(
        &path,
        element["at"].as_str().unwrap(),
        &destination,
        &["--content"],
    );
    assert!(
        content.status.success(),
        "{}",
        String::from_utf8_lossy(&content.stderr)
    );
    assert_ne!(fs::read(&destination).unwrap(), b"old");
}

#[test]
fn html_extract_content_preserves_inline_script_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("page.html");
    fs::write(
        &path,
        b"<!DOCTYPE html><script>const answer = 40 + 2; function run(v){return v;}</script>",
    )
    .unwrap();
    let catalog = rows(&run(&path, &[]));
    let script = catalog
        .iter()
        .find(|r| r["detail"]["tag"] == "script")
        .unwrap();
    let destination = dir.path().join("inline.js");
    receipt(&extract(
        &path,
        script["at"].as_str().unwrap(),
        &destination,
        &["--content"],
    ));
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"const answer = 40 + 2; function run(v){return v;}"
    );
}

fn rows_file_success(output: &Output) -> bool {
    output.status.success() && output.stdout.is_empty()
}

#[test]
fn html_list_preserves_lexical_recovery_and_corrected_noframes_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recover.html");
    fs::write(&path, b"<ul><li><li>x</ul></absent><noframes><b>hidden</b></noframes><noframe><i>visible</i></noframe>").unwrap();
    let entries = rows(&run(&path, &[]));
    let nodes = &entries[..];
    let tags: Vec<_> = nodes
        .iter()
        .filter(|row| row["record"] == "element")
        .map(|row| row["detail"]["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["ul", "li", "li", "noframes", "noframe", "i"]);
    assert_eq!(nodes[1]["parent_at"], nodes[0]["at"]);
    assert_eq!(nodes[2]["parent_at"], nodes[1]["at"]);
    assert_eq!(nodes[2]["detail"]["end_tag"], "missing");
    assert_eq!(nodes[1]["detail"]["end_tag"], "missing");
    assert_eq!(nodes[0]["detail"]["end_tag"], "explicit");
    assert_eq!(
        nodes
            .iter()
            .find(|row| row["detail"]["value"] == "<b>hidden</b>")
            .unwrap()["record"],
        "text"
    );
    fs::write(&path, b"<div><script src=unq").unwrap();
    let destination = dir.path().join("previous.jsonl");
    fs::write(&destination, b"prior catalog").unwrap();
    fails(
        run(&path, &["-o", destination.to_str().unwrap()]),
        "INVALID_INPUT",
    );
    assert_eq!(fs::read(destination).unwrap(), b"prior catalog");
    let absent = dir.path().join("not-created.jsonl");
    fails(
        run(&path, &["-o", absent.to_str().unwrap()]),
        "INVALID_INPUT",
    );
    assert!(!absent.exists());
}

#[test]
fn html_list_recovers_void_slash_ancestor_and_eof_ranges_without_losing_nul() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recovery.html");
    let source = include_bytes!("../fixtures/html/recovery.html");
    fs::write(&path, source).unwrap();
    let entries = rows(&run(&path, &[]));
    let nodes = &entries[..];
    let elements: Vec<_> = nodes.iter().filter(|r| r["record"] == "element").collect();
    assert_eq!(elements.len(), 5);
    for (row, tag, start, end, content_start, content_end, status, slash, void) in [
        (&elements[0], "div", 0, 42, 5, 36, "explicit", false, false),
        (
            &elements[1],
            "br",
            5,
            10,
            10,
            10,
            "not_applicable",
            true,
            true,
        ),
        (
            &elements[2],
            "section",
            10,
            36,
            20,
            36,
            "missing",
            true,
            false,
        ),
        (
            &elements[3],
            "span",
            20,
            36,
            26,
            29,
            "explicit",
            false,
            false,
        ),
        (&elements[4], "p", 51, 58, 54, 58, "missing", false, false),
    ] {
        assert_eq!(row["detail"]["tag"], tag);
        assert_eq!(
            (row["start"].as_u64().unwrap(), row["end"].as_u64().unwrap()),
            (start, end)
        );
        assert_eq!(row["detail"]["content_start"], content_start);
        assert_eq!(row["detail"]["content_end"], content_end);
        assert_eq!(row["detail"]["end_tag"], status);
        assert_eq!(row["detail"]["self_closing"], slash);
        assert_eq!(row["detail"]["void"], void);
    }
    assert_eq!(elements[1]["parent_at"], elements[0]["at"]);
    assert_eq!(elements[2]["parent_at"], elements[0]["at"]);
    assert_eq!(elements[3]["parent_at"], elements[2]["at"]);
    assert!(elements[4]["parent_at"].is_null());
    let nul_text = nodes
        .iter()
        .find(|r| r["detail"]["value"] == "A\0B")
        .unwrap();
    assert_eq!(
        (
            nul_text["start"].as_u64().unwrap(),
            nul_text["end"].as_u64().unwrap()
        ),
        (26, 29)
    );
    assert_eq!(&source[26..29], b"A\0B");
    assert_eq!(nul_text["parent_at"], elements[3]["at"]);
}

#[test]
fn html_list_uses_context_states_without_inventing_markup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("contexts.html");
    fs::write(&path, include_bytes!("../fixtures/html/contexts.html")).unwrap();
    let entries = rows(&run(&path, &[]));
    let nodes = &entries[..];
    let elements: Vec<_> = nodes.iter().filter(|r| r["record"] == "element").collect();
    let tags: Vec<_> = elements
        .iter()
        .map(|r| r["detail"]["tag"].as_str().unwrap())
        .collect();
    assert_eq!(
        tags,
        [
            "textarea",
            "title",
            "style",
            "noscript",
            "noframes",
            "noframe",
            "i",
            "plaintext"
        ]
    );
    for (tag, value) in [
        ("textarea", "<b>t</b>"),
        ("title", "<i>x</i>"),
        ("style", "<b>s</b>"),
        ("noscript", "<b>n</b>"),
        ("noframes", "<b>f</b>"),
        ("plaintext", "<b>p</b>"),
    ] {
        let parent = elements.iter().find(|r| r["detail"]["tag"] == tag).unwrap();
        let text = nodes
            .iter()
            .find(|r| r["detail"]["value"] == value)
            .unwrap();
        assert_eq!(text["record"], "text");
        assert_eq!(text["parent_at"], parent["at"]);
    }
    assert_eq!(elements[6]["parent_at"], elements[5]["at"]);
    assert_eq!(elements[7]["detail"]["end_tag"], "missing");
    assert_eq!(elements[7]["end"], 179);
}

#[test]
fn html_list_declares_context_approximation_in_cli_discovery_and_results() {
    let discovery = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["--help"])
        .output()
        .unwrap();
    assert!(discovery.status.success());
    assert!(String::from_utf8_lossy(&discovery.stdout).contains("lexical_naive_v1"));
    let help = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["html", "entries", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("lexical_naive_v1"));
    assert!(!String::from_utf8_lossy(&help.stdout).contains("DOM-complete"));

    let dir = tempfile::tempdir().unwrap();
    for (fixture, expected_tags, hidden) in [
        ("foreign.html", vec!["svg", "script"], "<p>later</p>"),
        ("template.html", vec!["template", "tr", "td", "p"], ""),
        ("script-data.html", vec!["script", "p"], "<b>still text</b>"),
    ] {
        let path = dir.path().join(fixture);
        fs::write(
            &path,
            fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/html")
                    .join(fixture),
            )
            .unwrap(),
        )
        .unwrap();
        let entries = rows(&run(&path, &[]));
        let nodes = &entries[..];
        let elements: Vec<_> = nodes.iter().filter(|r| r["record"] == "element").collect();
        assert_eq!(
            elements
                .iter()
                .map(|r| r["detail"]["tag"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected_tags
        );
        if !hidden.is_empty() {
            assert!(nodes.iter().any(|r| r["record"] == "text"
                && r["detail"]["value"].as_str().unwrap().contains(hidden)));
        }
        if fixture == "foreign.html" {
            assert_eq!(
                (elements[0]["start"].as_u64(), elements[0]["end"].as_u64()),
                (Some(0), Some(37))
            );
            assert_eq!(
                (elements[1]["start"].as_u64(), elements[1]["end"].as_u64()),
                (Some(5), Some(37))
            );
            assert_eq!(elements[1]["detail"]["content_start"], 13);
            assert_eq!(elements[1]["detail"]["content_end"], 37);
            assert_eq!(elements[1]["parent_at"], elements[0]["at"]);
            assert_eq!(elements[0]["detail"]["end_tag"], "missing");
        } else if fixture == "template.html" {
            assert_eq!(elements[1]["parent_at"], elements[0]["at"]);
            assert_eq!(elements[2]["parent_at"], elements[1]["at"]);
            assert!(elements[3]["parent_at"].is_null());
        } else {
            assert_eq!(elements[0]["detail"]["end_tag"], "explicit");
            assert_eq!(
                (elements[0]["start"].as_u64(), elements[0]["end"].as_u64()),
                (Some(0), Some(61))
            );
            assert_eq!(elements[0]["detail"]["content_start"], 8);
            assert_eq!(elements[0]["detail"]["content_end"], 52);
            assert_eq!(
                (elements[1]["start"].as_u64(), elements[1]["end"].as_u64()),
                (Some(61), Some(73))
            );
            assert!(elements[1]["parent_at"].is_null());
        }
    }
}

#[test]
fn html_list_keeps_nonvoid_slash_tag_open_until_eof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.html");
    fs::write(&path, b"<div/>").unwrap();
    let entries = rows(&run(&path, &[]));
    assert_eq!(entries[0]["detail"]["self_closing"], true);
    assert_eq!(entries[0]["detail"]["void"], false);
    assert_eq!(entries[0]["detail"]["end_tag"], "missing");
    assert_eq!(entries[0]["detail"]["content_start"], 6);
    assert_eq!(entries[0]["detail"]["content_end"], 6);
}

#[test]
fn html_list_rejects_untrusted_inputs_and_limits_without_replacing_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input");
    let target = dir.path().join("catalog");
    fs::write(&target, b"old").unwrap();
    fs::write(&path, b"<a><b>text</b></a>").unwrap();
    for flag in [
        "--max-input-bytes",
        "--max-records",
        "--max-depth",
        "--max-output-bytes",
    ] {
        let mut args = vec![flag, "1"];
        args.extend(["-o", target.to_str().unwrap()]);
        fails(run(&path, &args), "unexpected argument");
        assert_eq!(fs::read(&target).unwrap(), b"old");
    }
    fs::write(&path, [0xff]).unwrap();
    fails(
        run(&path, &["-o", target.to_str().unwrap()]),
        "INVALID_INPUT",
    );
    fails(run(dir.path(), &[]), "INVALID_INPUT");
    assert_eq!(fs::read(target).unwrap(), b"old");
    fs::write(&path, b"").unwrap();
    let empty = rows(&run(&path, &[]));
    assert!(empty.is_empty());

    let malformed = run(&path, &["--max-records", "0"]);
    assert_eq!(malformed.status.code(), Some(2));
    assert!(malformed.stdout.is_empty());

    let measured = run(&path, &["--perf"]);
    assert!(measured.status.success());
    let perf: Value =
        serde_json::from_slice(measured.stderr.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(perf["backend"], "html");
    assert_eq!(perf["command"], "entries");
    assert_eq!(perf["counters"]["records"], 0);
    assert_eq!(perf["counters"]["input_bytes"], 0);
    for stage in [
        "read",
        "hash",
        "utf8",
        "tokenize",
        "query",
        "serialize",
        "write",
    ] {
        assert!(
            perf["stages_ns"][stage].is_number(),
            "missing stage {stage}"
        );
    }
}
