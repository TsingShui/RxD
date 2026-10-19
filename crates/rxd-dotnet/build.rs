use std::{
    env,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

fn sha256(bytes: impl AsRef<[u8]>) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    hex
}

fn checked_bytes(path: &Path, maximum: u64) -> Vec<u8> {
    println!("cargo:rerun-if-changed={}", path.display());
    let metadata = fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("PREBUILT_ILSPY: cannot read {}: {error}", path.display()));
    assert!(
        metadata.is_file() && metadata.len() <= maximum,
        "PREBUILT_ILSPY: invalid file {}",
        path.display()
    );
    fs::read(path).expect("read prebuilt ILSpy file")
}

fn source_paths(directory: &Path, paths: &mut Vec<PathBuf>, depth: usize) {
    assert!(depth <= 64, "PREBUILT_ILSPY: excessive source nesting");
    for entry in fs::read_dir(directory).expect("read native source directory") {
        let entry = entry.expect("read native source entry");
        let kind = entry.file_type().unwrap();
        assert!(
            !kind.is_symlink(),
            "PREBUILT_ILSPY: symlinked native source"
        );
        let path = entry.path();
        if kind.is_dir() {
            source_paths(&path, paths, depth + 1);
        } else if path.extension().is_some_and(|extension| extension == "cs") {
            paths.push(path);
            assert!(
                paths.len() <= 4096,
                "PREBUILT_ILSPY: excessive source count"
            );
        }
    }
}

fn prebuilt_embed() -> PathBuf {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let target = env::var("TARGET").unwrap();
    let directory = root.join("prebuilt/ilspy");
    let index: Value =
        serde_json::from_slice(&checked_bytes(&directory.join("index.json"), 1024 * 1024))
            .expect("PREBUILT_ILSPY: invalid index JSON");
    assert_eq!(
        index["schema_version"], 1,
        "PREBUILT_ILSPY: unsupported index schema"
    );
    let entry = index["artifacts"].get(&target).unwrap_or_else(|| {
        panic!("PREBUILT_ILSPY: no verified artifact for {target}. Build the native engine once on that OS with scripts/build-ilspy.py, then import it with scripts/prebuilt-ilspy.py add; or explicitly set RXD_ILSPY_EMBED_RS. Cargo never downloads or builds .NET.")
    });
    let artifact = directory.join(&target);
    let manifest_path = artifact.join("manifest.json");
    let manifest_bytes = checked_bytes(&manifest_path, 4 * 1024 * 1024);
    assert_eq!(
        entry["manifest_sha256"].as_str(),
        Some(sha256(&manifest_bytes).as_str()),
        "PREBUILT_ILSPY: manifest checksum mismatch"
    );
    let manifest: Value =
        serde_json::from_slice(&manifest_bytes).expect("PREBUILT_ILSPY: invalid manifest JSON");
    assert_eq!(
        manifest["schema_version"], 1,
        "PREBUILT_ILSPY: unsupported manifest schema"
    );
    assert_eq!(
        manifest["target"].as_str(),
        Some(target.as_str()),
        "PREBUILT_ILSPY: target mismatch"
    );
    let inputs = entry["source_inputs"]
        .as_object()
        .expect("PREBUILT_ILSPY: missing source identity");
    let native = root.join("crates/rxd-dotnet/native");
    println!("cargo:rerun-if-changed={}", native.display());
    let mut paths = vec![
        native.join("rxd-dotnet-native.csproj"),
        native.join("NOTICE"),
    ];
    source_paths(&native, &mut paths, 0);
    assert_eq!(
        inputs.len(),
        paths.len(),
        "PREBUILT_ILSPY: stale source inventory"
    );
    for path in paths {
        let relative = path
            .strip_prefix(&root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let hash = sha256(checked_bytes(&path, 4 * 1024 * 1024));
        assert_eq!(
            inputs.get(&relative).and_then(Value::as_str),
            Some(hash.as_str()),
            "PREBUILT_ILSPY: stale {relative}; rebuild and reimport the native artifact"
        );
    }
    assert_eq!(
        manifest["native_build"]["source_inputs"], entry["source_inputs"],
        "PREBUILT_ILSPY: native build/source mismatch"
    );
    let bundle = artifact.join("bundle.tar.gz");
    let bundle_bytes = checked_bytes(&bundle, 64 * 1024 * 1024);
    assert_eq!(
        manifest["bundle_bytes"].as_u64(),
        Some(bundle_bytes.len() as u64),
        "PREBUILT_ILSPY: bundle size mismatch"
    );
    let hash = sha256(bundle_bytes);
    assert_eq!(
        entry["bundle_sha256"].as_str(),
        Some(hash.as_str()),
        "PREBUILT_ILSPY: bundle checksum mismatch"
    );
    assert_eq!(
        manifest["bundle_sha256"].as_str(),
        Some(hash.as_str()),
        "PREBUILT_ILSPY: archive/manifest mismatch"
    );
    let embed = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("ilspy-prebuilt.rs");
    fs::write(&embed, format!(
        "pub const DATA: &[u8] = include_bytes!({:?});\npub const MANIFEST: &str = include_str!({:?});\npub const TARGET: &str = {:?};\n",
        bundle.canonicalize().unwrap().to_str().unwrap(),
        manifest_path.canonicalize().unwrap().to_str().unwrap(), target,
    )).expect("write prebuilt ILSpy includes");
    embed
}

fn main() {
    println!("cargo:rerun-if-env-changed=RXD_DOTNET_NATIVE_LIBRARY");
    if env::var_os("CARGO_FEATURE_CSHARP_DECOMPILER").is_none() {
        return;
    }
    assert!(
        env::var("CARGO_CFG_TARGET_OS").unwrap() != "macos"
            || env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "aarch64",
        "csharp-decompiler: macOS supports ARM64 only"
    );
    println!("cargo:rerun-if-env-changed=RXD_ILSPY_EMBED_RS");
    if env::var_os("CARGO_FEATURE_BUNDLED_ILSPY").is_some() {
        let embed = env::var_os("RXD_ILSPY_EMBED_RS")
            .map(PathBuf::from)
            .unwrap_or_else(prebuilt_embed);
        assert!(
            embed.is_file(),
            "bundled-ilspy: generated embed.rs is missing"
        );
        println!("cargo:rerun-if-changed={}", embed.display());
        println!("cargo:rustc-env=RXD_ILSPY_EMBED_RS={}", embed.display());
        return;
    }
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        arch => panic!("csharp-decompiler: unsupported native architecture {arch}"),
    };
    let (platform, filename) = match env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "macos" => ("osx", "librxd_dotnet_native.dylib"),
        "linux" => ("linux", "librxd_dotnet_native.so"),
        "windows" => ("win", "rxd_dotnet_native.dll"),
        platform => panic!("csharp-decompiler: unsupported native platform {platform}"),
    };
    let library = env::var_os("RXD_DOTNET_NATIVE_LIBRARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join(format!(
                "../../target/dotnet-native/{platform}-{arch}/{filename}"
            ))
        });
    assert!(
        library.is_file(),
        "csharp-decompiler: missing native library {}. Run python3 scripts/build-ilspy.py {platform}-{arch}, or set RXD_DOTNET_NATIVE_LIBRARY to a prebuilt dylib. Cargo never invokes dotnet.",
        library.display()
    );
    let library = library.canonicalize().expect("resolve native library path");
    println!("cargo:rerun-if-changed={}", library.display());
    println!(
        "cargo:rustc-env=RXD_DOTNET_NATIVE_LIBRARY={}",
        library.display()
    );
    println!("cargo:rustc-env=RXD_DOTNET_LIBRARY_NAME={filename}");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile = output.ancestors().nth(3).expect("Cargo profile directory");
    println!("cargo:rustc-env=RXD_DOTNET_BUILD_DIR={}", profile.display());
    // No rustc-link-lib: all CLI commands must start even when the engine is missing.
}
