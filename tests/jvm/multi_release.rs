use super::*;

#[test]
fn nested_multi_release_selection_matches_scoped_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("library.aar");
    let data = fs::read("tests/corpus/jvm/gson-2.11.0.jar").unwrap();
    let mut jar = zip::ZipArchive::new(Cursor::new(data)).unwrap();
    let mut base = Vec::new();
    std::io::Read::read_to_end(
        &mut jar.by_name("com/google/gson/GsonBuilder.class").unwrap(),
        &mut base,
    )
    .unwrap();
    let mut versioned = base.clone();
    let start = versioned
        .windows(b"serializeNulls".len())
        .position(|w| w == b"serializeNulls")
        .unwrap();
    versioned[start..start + b"serializeNulls".len()].copy_from_slice(b"serializeNulLs");
    let inner = archive(&[
        (
            "META-INF/MANIFEST.MF",
            b"Manifest-Version: 1.0\r\nMulti-Release: true\r\n\r\n",
        ),
        ("com/google/gson/GsonBuilder.class", &base),
        (
            "META-INF/versions/9/com/google/gson/GsonBuilder.class",
            &versioned,
        ),
    ]);
    fs::write(&input, archive(&[("classes.jar", &inner)])).unwrap();
    let at = "classes.jar!Lcom/google/gson/GsonBuilder;";
    for (release, name) in [("8", "serializeNulls"), ("9", "serializeNulLs")] {
        // The first UTF8 occurrence may be the field; inspect the Java class either way.
        let java = text(run(
            &input,
            "get-class",
            &["--at", at, "--release", release],
        ));
        assert!(java.contains(name), "{java}");
    }
}
