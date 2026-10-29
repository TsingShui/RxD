use std::{env, path::PathBuf};

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
    assert!(
        env::var("CARGO_CFG_TARGET_OS").unwrap() != "linux"
            || env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "musl",
        "csharp-decompiler: Linux engines require a matching *-unknown-linux-musl Rust target"
    );
    println!("cargo:rerun-if-env-changed=RXD_ILSPY_EMBED_RS");
    if env::var_os("CARGO_FEATURE_BUNDLED_ILSPY").is_some() {
        let embed = env::var_os("RXD_ILSPY_EMBED_RS").unwrap_or_else(|| {
            panic!(
                "bundled-ilspy requires RXD_ILSPY_EMBED_RS pointing at the embed.rs produced by scripts/build-ilspy.py <rid>; ILSpy runtimes are produced per target and are not checked in"
            )
        });
        let embed = PathBuf::from(embed);
        assert!(
            embed.is_file(),
            "bundled-ilspy: RXD_ILSPY_EMBED_RS is not a file; run scripts/build-ilspy.py <rid>"
        );
        let embed = embed.canonicalize().expect("resolve ILSpy embed path");
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
        "linux" => ("linux-musl", "librxd_dotnet_native.so"),
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
        "csharp-decompiler: missing native library {}. Run python3 scripts/build-ilspy.py {platform}-{arch}, or set RXD_DOTNET_NATIVE_LIBRARY to a produced dylib. Cargo never invokes dotnet.",
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
