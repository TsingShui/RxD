//! Exercise the bundled native renderer and its installed-library contract.
use super::*;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/dotnet/newtonsoft-json-13.0.3/Newtonsoft.Json.dll")
}

#[test]
fn dotnet_bundled_engine_returns_source_for_selected_methods_and_classes() {
    let directory = tempfile::tempdir().unwrap();
    for (path, command, at) in [
        (corpus(), "get-method", "0x060000fb"),
        (corpus(), "get-class", "0x02000022"),
        (
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/sugar/Sugar.dll"),
            "get-method",
            "0x06000001",
        ),
        (
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet/closure/Closure.dll"),
            "get-class",
            "0x02000004",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["ilspy", command])
            .arg(&path)
            .args(["--at", at, "--perf"])
            .env("RXD_CACHE_DIR", directory.path())
            .env_remove("RXD_DOTNET_NATIVE_LIBRARY")
            .env_remove("RXD_REFDIR")
            .output()
            .unwrap();
        success(&output);
        let source = String::from_utf8(output.stdout.clone()).unwrap();
        assert!(!source.trim().is_empty());
        assert!(source.contains("public "), "{source}");
        assert!(!source.contains("// max_stack="), "returned original IL");
        assert!(!source.contains("ILSpy could not decompile"), "{source}");
        let reports = perf_reports(&output);
        let analysis = reports
            .iter()
            .find(|report| report["scope"] == "analysis")
            .unwrap();
        assert_eq!(analysis["status"], "ok");
        assert_eq!(
            analysis["counters"]["output_bytes"],
            output.stdout.len() as u64
        );
        let destination = directory.path().join("source.cs");
        let written = Command::new(env!("CARGO_BIN_EXE_rxd"))
            .args(["ilspy", command])
            .arg(&path)
            .args(["--at", at, "-o"])
            .arg(&destination)
            .env("RXD_CACHE_DIR", directory.path())
            .env_remove("RXD_DOTNET_NATIVE_LIBRARY")
            .env_remove("RXD_REFDIR")
            .output()
            .unwrap();
        success(&written);
        assert!(written.stdout.is_empty());
        assert_eq!(fs::read(destination).unwrap(), output.stdout);
    }
}

#[test]
fn dotnet_relocated_bundle_is_lazy_and_rejects_corruption_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory
        .path()
        .join(if cfg!(windows) { "rxd.exe" } else { "rxd" });
    fs::copy(env!("CARGO_BIN_EXE_rxd"), &binary).unwrap();
    let cache = directory.path().join("cache");
    let path = corpus();
    let query = |command: &str, args: &[&str]| {
        Command::new(&binary)
            .args(["ilspy", command])
            .arg(&path)
            .args(args)
            .current_dir(directory.path())
            .env("RXD_CACHE_DIR", &cache)
            .env_remove("RXD_DOTNET_NATIVE_LIBRARY")
            .env_remove("RXD_REFDIR")
            .env_remove("DOTNET_ROOT")
            .env_remove("DOTNET_ROOT_ARM64")
            .env_remove("DOTNET_ROOT_X64")
            .output()
            .unwrap()
    };
    success(&query("classes", &["-f", "JsonReaderException"]));
    let raw = query("get-method", &["--at", "0x060000fb", "--raw"]);
    success(&raw);
    assert!(!cache.join("rxd-ilspy").exists());
    let source = query("get-method", &["--at", "0x060000fb"]);
    success(&source);
    assert_ne!(source.stdout, raw.stdout);
    let runtimes: Vec<_> = fs::read_dir(cache.join("rxd-ilspy"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join(".complete").is_file())
        .collect();
    assert_eq!(runtimes.len(), 1);
    let runtime = &runtimes[0];
    assert!(runtime.join("dotnet-refs/System.Runtime.dll").is_file());
    let library = runtime.join(if cfg!(windows) {
        "rxd_dotnet_native.dll"
    } else if cfg!(target_os = "macos") {
        "librxd_dotnet_native.dylib"
    } else {
        "librxd_dotnet_native.so"
    });
    fs::write(library, b"corrupt").unwrap();
    assert_eq!(
        query("get-method", &["--at", "0x060000fb", "--raw"]).stdout,
        raw.stdout
    );
    let destination = directory.path().join("preserved.cs");
    fs::write(&destination, b"preserved").unwrap();
    let output = query(
        "get-method",
        &["--at", "0x060000fb", "-o", destination.to_str().unwrap()],
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("integrity verification"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(destination).unwrap(), b"preserved");
}
