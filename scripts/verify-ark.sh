#!/usr/bin/env bash
# Bounded Ark verification for completion audits; offline, never executes input.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
python3 scripts/format-workspace.py --check
cargo clippy -p rxd-ark -p rxd --all-targets -- -D warnings
python3 scripts/gen-abc-opcodes.py --check
if [[ -n "${ARK_ABC:-}" ]]; then
    mkdir -p target/verification/ark
    cargo run --quiet --release -p rxd-ark --example audit -- "$ARK_ABC" > target/verification/ark/verify-audit.json
fi
