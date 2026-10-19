//! The contract library consumers (rxd) embed.
//!
//! This test only uses the facade, so it must compile and pass with
//! `--no-default-features`, where the crate has no dependencies at all:
//!
//! ```text
//! cargo test --no-default-features --test lib_api
//! ```
//!
//! Keeping it feature-free is the point: anything it needs that pulls in the
//! `cli` feature or a C dependency is a facade leak.

use std::sync::Arc;

use radguard::{Decompiler, DexFile};

fn sample_bytes() -> Vec<u8> {
    std::fs::read("tests/fixtures/sample.dex").expect("read phase-one sample")
}

fn first_descriptor(dex: &DexFile<'_>) -> String {
    let class = &dex.classes()[0];
    dex.class_descriptor(class)
        .expect("descriptor of the first class")
        .to_owned()
}

#[test]
fn decompiles_one_class_from_borrowed_bytes() {
    let bytes = sample_bytes();
    let decompiler = Decompiler::parse(&bytes).expect("parse sample.dex");
    let descriptor = first_descriptor(decompiler.dex());

    let output = decompiler
        .decompile_class(&descriptor)
        .expect("decompile the first class")
        .expect("the first class is defined in this DEX");

    assert!(
        output.source.contains("class "),
        "rendered source looks wrong"
    );
    assert!(
        output
            .member_references
            .iter()
            .all(|reference| reference.start <= reference.end),
        "member reference spans must be ordered"
    );
}

#[test]
fn from_arc_matches_borrowed_parsing() {
    let bytes = sample_bytes();
    let descriptor = first_descriptor(&DexFile::parse(&bytes).expect("parse sample.dex"));

    let borrowed = Decompiler::parse(&bytes).expect("parse sample.dex");
    let shared = Decompiler::from_arc(Arc::from(bytes.clone())).expect("parse shared sample.dex");

    let from_borrowed = borrowed
        .decompile_class(&descriptor)
        .expect("decompile from borrowed bytes");
    let from_shared = shared
        .decompile_class(&descriptor)
        .expect("decompile from shared bytes");

    assert_eq!(from_borrowed, from_shared);
}

#[test]
fn unknown_descriptors_are_not_errors() {
    let bytes = sample_bytes();
    let decompiler = Decompiler::parse(&bytes).expect("parse sample.dex");

    for descriptor in [
        "Ldoes/not/Exist;",
        "",
        "not-a-descriptor",
        "Lcom/example/MainActivity;",
    ] {
        assert_eq!(
            decompiler
                .decompile_class(descriptor)
                .expect("a missing class is not a failure"),
            None,
            "{descriptor} should not resolve to a class"
        );
    }
}

#[test]
fn annotations_can_be_disabled_without_losing_the_class() {
    let bytes = sample_bytes();
    let descriptor = first_descriptor(&DexFile::parse(&bytes).expect("parse sample.dex"));

    let annotated = Decompiler::parse(&bytes)
        .expect("parse sample.dex")
        .decompile_class(&descriptor)
        .expect("decompile the first class")
        .expect("class is defined");
    let plain = Decompiler::parse(&bytes)
        .expect("parse sample.dex")
        .without_metadata()
        .decompile_class(&descriptor)
        .expect("decompile the first class")
        .expect("class is defined");

    assert!(!plain.source.is_empty());
    assert_ne!(annotated.source, plain.source, "annotations are opt-out");

    // Annotation-free output is shorter, so spans move; the references describe
    // the same members in the same order either way.
    let identity = |output: &radguard::SourceOutput| {
        output
            .member_references
            .iter()
            .map(|reference| {
                (
                    reference.kind.clone(),
                    reference.class.clone(),
                    reference.name.clone(),
                    reference.descriptor.clone(),
                    reference.declaration,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(identity(&annotated), identity(&plain));
}

#[test]
fn decompile_all_is_deterministic() {
    let bytes = sample_bytes();
    let decompiler = Decompiler::parse(&bytes).expect("parse sample.dex");

    let first = decompiler.decompile_all().expect("decompile every class");
    assert_eq!(first.len(), decompiler.dex().classes().len());
    assert!(
        first
            .iter()
            .all(|(descriptor, output)| !descriptor.is_empty() && !output.source.is_empty())
    );

    let second = decompiler
        .decompile_all()
        .expect("decompile every class again");
    assert_eq!(
        first, second,
        "one decompiler is reusable and deterministic"
    );
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    let bytes = sample_bytes();

    for length in [0, 1, 7, 8, 63, 112, 4096] {
        let truncated = &bytes[..length];
        assert!(
            Decompiler::parse(truncated).is_err(),
            "truncated DEX of {length} bytes must be rejected"
        );
    }

    let mut garbage = bytes.clone();
    for (offset, byte) in garbage.iter_mut().enumerate().take(4096).step_by(7) {
        *byte = (offset as u8).wrapping_mul(31);
    }
    // Corrupting the index tables can still leave a parseable header; rendering
    // must not panic, whatever it produces.
    if let Ok(decompiler) = Decompiler::parse(&garbage) {
        let _ = decompiler.decompile_all();
    }
}
