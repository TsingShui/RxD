use std::io::{Cursor, Write};

use rxd_core::{Budget, Failure, perf::Perf};
use rxd_zip::{Options, Query};
use zip::{CompressionMethod, ZipWriter, write::FileOptions};

#[test]
fn explicit_caller_quotas_stay_independent_of_compressed_input_size() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.zip");
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["one", "two"] {
        writer
            .start_file(
                name,
                FileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(&[b'x'; 4096]).unwrap();
    }
    std::fs::write(&input, writer.finish().unwrap().into_inner()).unwrap();
    let base = Budget::for_input_bytes(std::fs::metadata(&input).unwrap().len() as usize).unwrap();
    let mut perf = Perf::new(false, "analysis", "zip", "test");
    let selected = rxd_zip::read_entry(
        "one",
        Options {
            input: &input,
            output: None,
            budget: base,
        },
        &mut perf,
    )
    .unwrap();
    assert_eq!(selected.bytes, vec![b'x'; 4096]);
    for budget in [
        Budget { entries: 1, ..base },
        Budget {
            metadata_bytes: 1,
            ..base
        },
        Budget {
            entry_bytes: Some(4095),
            ..base
        },
        Budget {
            input_bytes: 1,
            ..base
        },
    ] {
        let error = rxd_zip::read_entry(
            "one",
            Options {
                input: &input,
                output: None,
                budget,
            },
            &mut perf,
        )
        .err()
        .unwrap();
        assert_eq!(error.kind(), Some(Failure::Resource), "{error}");
    }
    let error = rxd_zip::run(
        Query::Entries {
            contains: None,
            case_sensitive: false,
        },
        Options {
            input: &input,
            output: None,
            budget: Budget {
                output_bytes: Some(1),
                ..base
            },
        },
        &mut perf,
    )
    .err()
    .unwrap();
    assert_eq!(error.kind(), Some(Failure::Resource));
}
