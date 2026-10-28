#!/usr/bin/env bash
# Bounded Ark verification for completion audits; offline, never executes input.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
python3 scripts/format-workspace.py --check
cargo test -p rxd --no-default-features --features backend-abc --test cli abc::
cargo clippy -p rxd-abc -p rxd --all-targets -- -D warnings
python3 scripts/gen-abc-opcodes.py --check
if [[ -n "${ARK_ABC:-}" ]]; then
    cargo run --quiet --release -p rxd-abc --example audit -- "$ARK_ABC" --content-check
fi
