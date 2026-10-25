# Scoped validation

Select validation by changed behavior, not by the fact that a commit or merge is
being made. This policy replaces the former unconditional pre-commit workspace
Clippy + all-backend debug/release rule. Correctness requirements are unchanged;
the 150 ms query gate is scoped to decompilation/analysis backend operations, not
test-suite wall time or every executable command. Installation/configuration,
skill printing, help/version, standalone MCP discovery/lifecycle,
maintenance and build/test/release orchestration have no 150 ms acceptance gate.
Do not require their timing batches or numeric exceptions just to merge; optional
measurements do not replace functional/safety E2E or explicit resource limits.
See `AGENTS.md` for the authoritative scope and unchanged backend exceptions.

## Shared workstation resource discipline

Keep the interactive machine responsive during local agent work. Default to
`CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=2`; run build, lint, debug tests, release
tests and benchmarks sequentially, not as overlapping background jobs. Use lower
OS scheduling priority for compilation and cleanup when available. Do not raise
concurrency without the operator's agreement. CI keeps its separately configured
parallelism. These are development-process controls, not backend query budgets.

After a validation milestone, stop only owned disposable workers/browsers and
remove known rebuildable duplicate target directories and stale incremental caches
when no build is using them. Preserve source changes, pinned inputs/provenance,
reports/logs and the minimal hashed binaries needed to reproduce measurements.
Record moved evidence paths before deleting a baseline build tree. Do not kill
other sessions' processes, delete unknown captures, or indiscriminately clear the
whole Cargo cache and force an expensive rebuild. Check resource use before the
next heavy gate; bounded, scope-appropriate validation takes priority over running
more gates concurrently.

Full query-result dumps are disposable after verification unless needed for a
concrete regression. Prefer compact hashes, counts, provenance and selected samples
over repeated large JSONL exports; record cleanup rather than claiming removed
output is still retained. Inspect and hash large files incrementally, not with
whole-file string reads. Output benchmarks must still include real writes, but
reuse or remove their temporary result files afterward.

## Local feedback and milestones

| Changed behavior | Inner loop | Before the affected milestone |
| --- | --- | --- |
| Documentation / project Skill | Review syntax, links and examples; `git diff --check` | Same; no Rust build |
| Maintenance Python | Relevant real-script contract scenario | Its offline contract suite; no product CLI |
| Backend algorithm | Relevant public package test / regression | Affected package suite; scoped CLI when product behavior changes |
| Backend CLI / output / budgets | Relevant real CLI scenario | Backend feature-scoped debug/release CLI, relevant lint/format |
| Shared core, dispatch, query or protocol | Smallest affected regression first | Default all-backend debug/release, relevant package tests and optional-feature checks |
| Native engine / bundle / packaging | Relevant adapter/script checks | Full native feature, relocation, installation and platform checks |
| Release | Not an inner-loop operation | All four native platforms and publication gates; never reuse unrelated assets |

For example, while changing the production .NET metadata/IL parser:

```sh
cargo test -p rxd-dotnet
cargo clippy -p rxd-dotnet --all-targets -- -D warnings
```

Do not build the NativeAOT/Kuna distribution to test parser-only algorithms.
When the production .NET behavior changes:

```sh
cargo test -p rxd --no-default-features --features backend-dotnet --test cli dotnet::
cargo test --release -p rxd --no-default-features --features backend-dotnet --test cli dotnet::
```

Default `cargo build/test` enables `all-feature` (all backend namespaces and MCP,
but not the optional Kuna engine/specs or desktop ILSpy). C# reconstruction needs
`bundled-ilspy` plus an `RXD_ILSPY_EMBED_RS` produced by
`scripts/build-ilspy.py <rid>`, or `csharp-decompiler` plus an external native
library. Use `--no-default-features --features all-backends` for the unbundled
all-backend variant, and add `mcp` explicitly for that transport variant. Quick CI
chooses those configurations deliberately; a default-build milestone must
actually exercise the complete default profile.

A Cargo test-name filter reduces **execution**, not necessarily compilation. Use
package selection and backend features too. Do not churn between gratuitous
feature combinations, disable overflow checks, remove failure cases, or substitute
warmed parser timings for release CLI performance evidence. Benchmarks remain
required for affected backend-analysis runtime behavior, not unrelated maintenance
scripts or non-backend commands merely because their executable code changed.
For auxiliary commands, benchmark only when explicitly requested or needed to
investigate a concrete regression; do not impose the backend 150 ms gate.

`python3 scripts/ci-scope.py --base HEAD` reports a conservative plan for staged,
unstaged and untracked non-ignored paths. For committed changes specify both
endpoints, e.g. `--base main --head HEAD`. It emits reasons, selected quick scopes
and whether full packaging is required; it does not run tests or cache success.
Review the plan: it is a CI routing aid, not a proof that every semantic dependency
was identified. Widen checks when a change crosses the listed consumers.
Unknown paths fall back to shared checks. RadGuard selects both DEX and JVM.
JS/ASAR still have unnamespaced root CLI scenarios and conservatively use the
shared suite; do not invent a substring filter that silently omits them.
Embedded `src/skills/rxd/SKILL.md` is product source, unlike project `.agents/`.

## Reuse evidence, not assumptions

A conflict-free merge with an identical validated source tree does not justify
rerunning the same gates locally. Reuse is explicit and gate-specific:

1. Identify the successful run, exact argv, selected tests/features/profile,
   toolchain, platform, relevant environment and dependency pins.
2. Prove all inputs match: Rust/Python source, tests, fixture bytes, embedded
   Markdown/data, gitlinks and initialized upstream state. For identical clean
   snapshots, `git rev-parse REV^{tree}` equality proves tracked-byte identity;
   it does not prove the environment or generated/untracked inputs match.
3. Match the actual generated bundle/native/reference manifests **and their file
   hashes**, plus toolchain/build configuration. A common path or unchanged
   Rust files alone is insufficient. Include any dirty/untracked test inputs.
4. Reuse only matching successful gates. Record their identities and why they
   apply; missing evidence, a conflict/resolution, different inputs, flags or
   environment means rerun the affected gates. Never report a reused run as new.

There is no new persistent pass cache or complex test scheduler. CI uses normal
build caches, not cached success. Do not run an unchanged suite once for a feature
commit and again solely to create a main merge commit. Full CI remains independent
validation; local reuse is not permission to bypass required hosted checks.
Live upstream/network results are time-sensitive and not reusable as current facts.

## Test design and measured cost

Retain real-process checks for CLI/protocol, identities, byte-exact extraction,
budgets, atomic failures and distribution. Test algorithms through public package
interfaces. Do not add mirrors of private helpers or duplicate every case at each
layer; each additional layer needs a distinct observable risk.

Measure slow tests by fixture preparation, subprocess and assertion work before
changing them. Prefer cheaper equivalent fixture construction and immutable
fixture reuse, with independent mutable test state. Avoid broad mocking,
unbounded parallelism and timing assertions in ordinary tests.

CI tiers and required-check migration are documented in [CI](../demo/ci.md).
