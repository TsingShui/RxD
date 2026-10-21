use std::{env, fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git is required to validate the pinned Kuna checkout");
    assert!(
        result.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn replace_once(source: &mut String, before: &str, after: &str) {
    assert_eq!(
        source.matches(before).count(),
        1,
        "Kuna adapter needs review: expected one {before:?}"
    );
    *source = source.replacen(before, after, 1);
}

fn write_changed(path: &Path, contents: &str) {
    if fs::read_to_string(path).ok().as_deref() != Some(contents) {
        fs::write(path, contents).unwrap();
    }
}

fn module_path(path: &Path, name: &str) -> String {
    format!("#[path = {:?}]\nmod {name};", path.to_str().unwrap())
}

fn watch(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            watch(&entry.unwrap().path());
        }
    }
}

fn main() {
    let package = std::path::PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = package.join("../..").canonicalize().unwrap();
    let upstream = root.join("third_party/kuna");
    let source = upstream.join("decompiler/crates/kuna-cli/src");
    let output = std::path::PathBuf::from(env::var_os("OUT_DIR").unwrap());
    watch(&source);
    let git_directory =
        std::path::PathBuf::from(git(&upstream, &["rev-parse", "--absolute-git-dir"]));
    watch(&git_directory.join("HEAD"));
    watch(&upstream.join("VERSION"));
    let revision = git(&upstream, &["rev-parse", "HEAD"]);
    let index = git(&root, &["ls-files", "--stage", "third_party/kuna"]);
    assert_eq!(
        index.split_whitespace().nth(1),
        Some(revision.as_str()),
        "Kuna checkout must match the gitlink"
    );
    assert!(
        git(
            &upstream,
            &["status", "--porcelain", "--untracked-files=no"]
        )
        .is_empty(),
        "Kuna tracked source must be unmodified"
    );
    assert_eq!(
        git(&upstream, &["rev-parse", "--is-shallow-repository"]),
        "false",
        "Kuna versioning requires full history"
    );
    let major = fs::read_to_string(upstream.join("VERSION")).unwrap();
    let bump = git(&upstream, &["log", "-1", "--format=%H", "--", "VERSION"]);
    let minor = git(
        &upstream,
        &["rev-list", "--count", &format!("{bump}..HEAD")],
    );
    println!("cargo:rustc-env=KUNA_VERSION={}.{}", major.trim(), minor);

    let mut jobs = fs::read_to_string(source.join("jobs.rs")).unwrap();
    replace_once(
        &mut jobs,
        "let mut cmd = Command::new(exe);",
        "let mut cmd = Command::new(exe);\n        cmd.arg(\"kuna\");",
    );
    for name in ["scratch", "synth", "type_blocks", "wire"] {
        replace_once(
            &mut jobs,
            &format!("mod {name};"),
            &module_path(&source.join("jobs").join(format!("{name}.rs")), name),
        );
    }
    write_changed(&output.join("jobs.rs"), &jobs);

    let mut boundary = fs::read_to_string(source.join("output.rs")).unwrap();
    replace_once(&mut boundary, "pub fn emit_bytes(bytes: &[u8]) -> io::Result<()> {", "pub fn emit_bytes(bytes: &[u8]) -> io::Result<()> {\n    if let Some(result) = crate::capture_output(bytes) { return result; }");
    write_changed(&output.join("output.rs"), &boundary);

    // Path-qualified parent modules need explicit locations for their child files.
    let relocated: &[(&str, &[&str])] = &[
        ("callgraph", &["arm_closure", "plan"]),
        ("decompile", &["script"]),
        ("decompile_all", &["callee_first"]),
        ("fid", &["archive"]),
        ("strings", &["filter"]),
        ("test", &["baseline"]),
    ];
    for (parent, children) in relocated {
        let mut module = fs::read_to_string(source.join(format!("{parent}.rs"))).unwrap();
        for child in *children {
            replace_once(
                &mut module,
                &format!("mod {child};"),
                &module_path(&source.join(parent).join(format!("{child}.rs")), child),
            );
        }
        write_changed(&output.join(format!("{parent}.rs")), &module);
    }

    let mut entry = fs::read_to_string(source.join("main.rs")).unwrap();
    replace_once(
        &mut entry,
        "//! Command dispatch for the kuna CLI.",
        "// Generated from the unmodified Kuna CLI entry.",
    );
    replace_once(&mut entry, "use std::process::ExitCode;", "");
    replace_once(&mut entry, "fn main() -> ExitCode {\n    let args: Vec<String> = std::env::args().collect();", "/// Run the original CLI with argv, including its program name.\npub fn run(args: Vec<String>) -> i32 {");
    replace_once(
        &mut entry,
        "    match sub {\n        paths::DECOMP_DBG_SUBCOMMAND => return kuna_console::consolemain::main(args[1..].to_vec()),\n        paths::SLACOMP_SUBCOMMAND => return kuna_slacomp::slacompmain::main(args[1..].to_vec()),\n        _ => {}\n    }\n",
        "",
    );
    replace_once(&mut entry, "return ExitCode::from(2);", "return 2;");
    replace_once(&mut entry, "ExitCode::from(code as u8)", "code");
    let generated = entry
        .lines()
        .map(|line| {
            if let Some(name) = line.strip_prefix("mod ").and_then(|s| s.strip_suffix(';')) {
                let directory = if matches!(name, "jobs" | "output")
                    || relocated.iter().any(|(parent, _)| *parent == name)
                {
                    &output
                } else {
                    &source
                };
                module_path(&directory.join(format!("{name}.rs")), name)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    write_changed(&output.join("cli.rs"), &generated);
}
