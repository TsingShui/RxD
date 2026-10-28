//! Static .NET CLI contracts using independently assembled original fixtures.
use super::*;

mod field_type_refs;
mod metadata_budget;
mod method_refs;
mod strings;

#[test]
fn enabled_backend_features_define_the_cli_catalog() {
    let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .arg("--help")
        .output()
        .unwrap();
    success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    let expected = [
        ("rasc", cfg!(feature = "backend-dex")),
        ("jar", cfg!(feature = "backend-jvm")),
        ("ilspy", cfg!(feature = "backend-dotnet")),
        ("evmole", cfg!(feature = "backend-evm")),
        ("unluac-rs", cfg!(feature = "backend-lua")),
        ("dae", cfg!(feature = "backend-flutter")),
        ("ark", cfg!(feature = "backend-abc")),
        ("wasm", cfg!(feature = "backend-wasm")),
        ("pyc", cfg!(feature = "backend-pyc")),
        ("kuna", cfg!(feature = "backend-kuna")),
        ("mcp", cfg!(feature = "mcp")),
    ];
    for (name, enabled) in expected {
        let listed = help
            .lines()
            .any(|line| line.trim_start().starts_with(&format!("{name} ")));
        assert_eq!(
            listed, enabled,
            "unexpected help visibility for {name}:\n{help}"
        );
    }
}

fn input() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/Fixture.dll")
}

fn expected() -> Value {
    serde_json::from_str(include_str!("../fixtures/dotnet/expected.json")).unwrap()
}

fn run(command: &str, path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ilspy", command])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn rejected(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "failure emitted partial stdout");
    assert!(!output.stderr.is_empty(), "failure omitted diagnostic");
}
#[test]
fn dotnet_coordinates_are_bare_metadata_tokens() {
    let path = input();
    for at in ["0x06000001", "06000001"] {
        success(&run("get-method", &path, &["--at", at, "--raw"]));
    }
    for at in ["0600001", "0x060000010", "0x601", "not-a-token"] {
        let out = run("get-method", &path, &["--at", at, "--raw"]);
        assert_eq!(out.status.code(), Some(2), "{at}");
        assert!(out.stdout.is_empty(), "{at} emitted partial stdout");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("INVALID_QUERY"),
            "{at}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // A leftover kind prefix is refused with the value that would work.
    for at in ["method:06000001", "token:0x06000001"] {
        let out = run("get-method", &path, &["--at", at, "--raw"]);
        assert_eq!(out.status.code(), Some(2), "{at}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("metadata token alone"), "{at}: {stderr}");
        assert!(stderr.contains("06000001"), "{at}: {stderr}");
    }
}

#[test]
fn dotnet_outline_ends_a_closed_pipe_quietly() {
    let path = input();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rxd"))
        .args(["ilspy", "methods"])
        .arg(&path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    {
        use std::io::Read;
        let mut first = [0u8; 1];
        stdout.read_exact(&mut first).unwrap();
    }
    drop(stdout);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "a closed pipe must not print a diagnostic: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
// Relocate valid IL/EH by extending .text and updating its MethodDef RVA.
