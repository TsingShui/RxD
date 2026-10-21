//! Adapt the pinned upstream CLI without editing its checkout.
// Upstream unit tests require its own manifest/layout; rxd exercises this adapter
// through the real CLI integration suite, including when Clippy checks all targets.
#![cfg(not(test))]
// The included command modules retain upstream's lint policy. The build script
// and rxd's dispatch remain covered by the repository's normal Clippy checks.
#![allow(clippy::all)]
// Upstream expect attributes refer to its own clippy.toml, not rxd's policy.
#![allow(unfulfilled_lint_expectations)]

include!(concat!(env!("OUT_DIR"), "/cli.rs"));

struct CapturedOutput {
    bytes: Vec<u8>,
    limit: Option<usize>,
    exceeded: bool,
}

std::thread_local! {
    static CAPTURE: std::cell::RefCell<Option<CapturedOutput>> = const { std::cell::RefCell::new(None) };
}

fn capture_output(bytes: &[u8]) -> Option<std::io::Result<()>> {
    CAPTURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let capture = slot.as_mut()?;
        if capture
            .limit
            .is_some_and(|limit| bytes.len() > limit.saturating_sub(capture.bytes.len()))
        {
            capture.exceeded = true;
            return Some(Err(std::io::Error::other(
                "Kuna output byte budget exceeded",
            )));
        }
        capture.bytes.extend_from_slice(bytes);
        Some(Ok(()))
    })
}

fn capture(args: Vec<String>, limit: Option<usize>) -> (i32, CapturedOutput) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            CAPTURE.with(|slot| *slot.borrow_mut() = None);
        }
    }
    CAPTURE.with(|slot| {
        assert!(slot.borrow().is_none(), "nested Kuna stdout capture");
        *slot.borrow_mut() = Some(CapturedOutput {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        });
    });
    let _reset = Reset;
    let code = run(args);
    let result = CAPTURE.with(|slot| slot.borrow_mut().take().unwrap());
    (code, result)
}

/// Preserve the ordinary CLI's existing one-shot native stdout capture.
pub fn run_captured(args: Vec<String>) -> (i32, Vec<u8>) {
    let (code, output) = capture(args, None);
    (code, output.bytes)
}

/// Bounded native stdout capture for isolated MCP workers only. Overflow never
/// returns a partial successful document, including when upstream handles I/O errors.
pub fn run_captured_bounded(args: Vec<String>, limit: usize) -> std::io::Result<(i32, Vec<u8>)> {
    if limit == 0 {
        return Err(std::io::Error::other(
            "Kuna output byte budget must be positive",
        ));
    }
    let (code, output) = capture(args, Some(limit));
    if output.exceeded {
        return Err(std::io::Error::other("Kuna output byte budget exceeded"));
    }
    Ok((code, output.bytes))
}

pub fn engine_version() -> &'static str {
    env!("KUNA_VERSION")
}

/// The pinned upstream commit, so tool descriptions cannot go stale.
pub fn engine_revision() -> &'static str {
    env!("KUNA_REVISION")
}
/// Use the linked engine's metadata instead of a separate catalog helper.
pub fn catalog_json() -> String {
    use kuna_decomp::kuna_phases::{kuna_num_settables, kuna_settable_by_index};
    let documented: std::collections::BTreeSet<_> = (0..kuna_num_settables())
        .map(|i| kuna_settable_by_index(i).option)
        .collect();
    let registered: std::collections::BTreeSet<_> = kuna_decomp::options::KUNA_OPTION_NAMES
        .iter()
        .copied()
        .collect();
    assert_eq!(documented, registered, "catalog/registration drift");
    kuna_decomp::kuna_phases::emit_catalog_json(|_| None)
}
