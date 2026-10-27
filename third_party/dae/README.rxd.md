# dae (vendored)

Upstream: https://github.com/ejfkdev/dae — crate `dae-rs` 0.1.14, MIT.
This is the unmodified crates.io source except for one Cargo.toml edit.

**Changes:**

1. `serde_json` no longer enables `preserve_order`. That feature is
additive across a cargo build, so depending on the published crate silently
switched every `rxd` JSON object to insertion order and broke the documented
sorted-key output. The vendored copy keeps the analyzer and decompiler identical
while leaving rxd's serialization contract intact.

2. `Analyzer::build_functions` maps the per-function naming work in parallel with
rayon and merges in `BTreeMap` order. The work is string decoding and mangling
for every function (26.8k on a 14 MB app), and it runs once per listing; the
sequential merge keeps output byte-identical for any thread count.

3. `Analyzer` gains `new_without_names`, which skips the ep→name map. That map
costs a full naming pass (21 ms on a 14 MB app) and is read only by the r2/ida/asm
exporters; listings need `func_eps`, which is now built on its own.

4. `fill_snapshot` prints per-cluster timings under `DART_AOT_TIMINGS`, the only
way to see which cluster dominates (the 41k-string cluster is the largest).

5. `n_threads` uses the machine's cores (clamped to 32) instead of stopping at 8,
because whole-snapshot scans are independent per function.

6. `profile::detect` no longer prints the detected profile to stderr. The host
CLI owns diagnostics and reports the same facts from `rxd dae info`, so the
library-level notice only added an unparsable line to every run.
