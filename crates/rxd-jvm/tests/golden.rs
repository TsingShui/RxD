//! Golden class/method/instruction output. Review changes before regeneration:
//! RXD_JVM_UPDATE_GOLDEN=1 cargo test -p rxd-jvm --test golden

use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

use rxd_jvm::{Input, Limits, bytecode, render_class, render_method};

/// Fixed target release for multi-release JAR selection.
const RELEASE: u16 = 17;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/jvm")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Snapshot class rendering, method bodies and decoded listings, including errors.
fn snapshot(path: &Path) -> String {
    let mut out = String::new();
    let input = match Input::read(path, Limits::default(), RELEASE) {
        Ok(input) => input,
        Err(error) => {
            writeln!(out, "INPUT_ERROR: {error}").expect("string write");
            return out;
        }
    };
    let _ = writeln!(
        out,
        "# {} classes, {} bytes",
        input.classes.len(),
        input.file_bytes
    );
    for class in &input.classes {
        let _ = writeln!(out, "\n== class {} ==", class.class.name);
        match render_class(&input, &class.class.name, Limits::default()) {
            Ok(source) => out.push_str(&source),
            Err(error) => {
                let _ = writeln!(out, "DECOMPILE_ERROR: {error}");
            }
        }
        for (index, method) in class.class.methods.iter().enumerate() {
            let _ = writeln!(
                out,
                "\n== method {index} {} {} ==",
                method.name, method.descriptor
            );
            if method.code.is_none() {
                out.push_str("(no Code attribute)\n");
                continue;
            }
            let proto = method
                .descriptor
                .strip_prefix('(')
                .unwrap_or(&method.descriptor);
            let id = format!("L{};->{}({}", class.class.name, method.name, proto);
            match render_method(&input, &id, Limits::default()) {
                Ok(source) => out.push_str(&source),
                Err(error) => {
                    let _ = writeln!(out, "DECOMPILE_ERROR: {error}");
                }
            }
            match bytecode(&class.class, method) {
                Ok(instructions) => {
                    let _ = writeln!(out, "{instructions:#?}");
                }
                Err(error) => {
                    let _ = writeln!(out, "BYTECODE_ERROR: {error}");
                }
            }
        }
    }
    out
}

fn check(fixture: &str) {
    let path = fixtures_dir().join(fixture);
    let actual = snapshot(&path);
    let name = fixture.replace('/', "-");
    let golden = golden_dir().join(format!("{name}.txt"));
    if std::env::var_os("RXD_JVM_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden_dir()).expect("create golden directory");
        std::fs::write(&golden, &actual).expect("write golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).unwrap_or_else(|error| {
        panic!(
            "missing baseline {} ({error}); generate it with \
             RXD_JVM_UPDATE_GOLDEN=1 cargo test -p rxd-jvm --test golden",
            golden.display()
        )
    });
    if actual != expected {
        let first = actual
            .lines()
            .zip(expected.lines())
            .position(|(actual, expected)| actual != expected)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()));
        let shown = |text: &str| {
            text.lines()
                .nth(first)
                .unwrap_or("<end of file>")
                .to_owned()
        };
        panic!(
            "{fixture} no longer matches {}\nfirst difference at line {}:\n  \
             baseline: {}\n  current:  {}",
            golden.display(),
            first + 1,
            shown(&expected),
            shown(&actual),
        );
    }
}

#[test]
fn default_parent_is_unchanged() {
    check("fixture/DefaultParent.class");
}

#[test]
fn flow_fixture_is_unchanged() {
    check("fixture/FlowFixture.class");
}

#[test]
fn interface_super_fixture_is_unchanged() {
    check("fixture/InterfaceSuperFixture.class");
}

#[test]
fn modern_fixture_is_unchanged() {
    check("fixture/ModernFixture.class");
}

#[test]
fn stored_jar_is_unchanged() {
    check("flow-stored.jar");
}
