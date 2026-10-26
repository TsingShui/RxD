use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use rxd_core::{Limits, perf::Perf};
use rxd_pyinstaller::{detect, unpack};
use sha2::{Digest, Sha256};

fn listing(root: &Path) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path.strip_prefix(root).unwrap();
            let name = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.insert(name);
        }
    }
    files
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn extracts_the_pinned_pyinstaller_fixture() {
    let Ok(root) = std::env::var("RXD_PYINSTALLER_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let input = root.join("hello");
    assert!(input.is_file(), "fixture is missing: {}", input.display());
    assert!(detect(&input).unwrap());

    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("out");
    fs::create_dir(&output).unwrap();
    let mut perf = Perf::new(true, "analysis", "pyinstaller", "unpack");
    let summary = unpack(&input, &output, Limits::default(), 1 << 20, &mut perf).unwrap();

    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let receipt = &manifest["receipt"];
    assert_eq!(receipt["format"], "pyinstaller");
    assert_eq!(
        summary.python.as_deref(),
        receipt["python"].as_str(),
        "python version"
    );
    assert_eq!(summary.files, receipt["files"].as_u64().unwrap() as usize);
    assert_eq!(summary.bytes, receipt["bytes"].as_u64().unwrap());
    assert_eq!(
        summary.pyz_files,
        receipt["pyz_files"].as_u64().unwrap() as usize
    );
    assert_eq!(summary.pyz_bytes, receipt["pyz_bytes"].as_u64().unwrap());
    assert_eq!(
        summary.skipped,
        receipt["skipped"].as_u64().unwrap() as usize
    );

    let files = manifest["files"].as_array().unwrap();
    assert_eq!(files.len(), 135);
    assert_eq!(summary.files, files.len());
    let mut expected = BTreeSet::new();
    for file in files {
        let path = file["path"].as_str().unwrap();
        let bytes = fs::read(output.join(path)).unwrap_or_else(|error| panic!("{path}: {error}"));
        assert_eq!(
            bytes.len() as u64,
            file["bytes"].as_u64().unwrap(),
            "{path}"
        );
        assert_eq!(sha256(&bytes), file["sha256"].as_str().unwrap(), "{path}");
        expected.insert(path.to_owned());
    }
    assert_eq!(listing(&output), expected);

    let script = fs::read(output.join("hello.pyc")).unwrap();
    assert_eq!(&script[..4], &[0x61, 0x0d, 0x0d, 0x0a]);
    assert!(output.join("PYZ.pyz_extracted/json/__init__.pyc").is_file());
}
