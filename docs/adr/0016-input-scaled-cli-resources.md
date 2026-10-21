# ADR 0016: Input-scaled CLI resources, separate service quotas

Status: accepted.

## Context

Fixed totals such as one million work units or 128 MiB of metadata/output reject
otherwise valid ordinary queries merely because an input is larger. For example,
Newtonsoft.Json 13.0.3's pinned `lib/net6.0/Newtonsoft.Json.dll` (712,464 bytes,
SHA-256 `22c649f75fce5be7c7ccda8880473b634ef69ecf33f5d1ab8ad892caf47d5a07`)
previously failed a complete, no-match string-reference scan with
`RESOURCE_LIMIT: method code work`. The CLI should not require an Agent to tune
resource flags to ask that question.

Removing every validation or substituting `usize::MAX` is not the solution:
compressed data, shared structures and reconstruction can amplify small inputs.
MCP also needs service admission independent of the input a request supplies.

## Decision

Ordinary CLI dispatch uses `BudgetPolicy::InputScaled`. MCP's CLI adapter explicitly
uses `BudgetPolicy::Fixed(Budget::default())`; its snapshot, concurrency, deadline,
record, response and native-worker controls are unchanged. Library callers still
supply explicit `Limits`; their default constructors preserve the old fixed policy.
There are no new public CLI budget flags.

`Budget::for_input_bytes(N)` derives finite bounds from the admitted physical local
file size, not a compressed entry's declared inflated length. The allowances below
retain existing small-input headroom; they are **not fixed total CLI ceilings**:

| Dimension | CLI safety envelope |
| --- | --- |
| Input read | admitted file length; existing address/span bounds still apply |
| One expanded entry | min(256 MiB + 16N, u32::MAX) |
| Metadata/projection storage | 128 MiB + 64N |
| Complete bounded serialization | 128 MiB + 256N |
| Archive entries | 1,048,576 + N |
| Instructions per selected body | 100,000 + N |
| Cumulative analysis work | 1,000,000 + 128N |
| JS semantic nodes | 1,000,000 + 4N |

All arithmetic is checked and addressability overflow is `RESOURCE_LIMIT`, never
an unlimited sentinel. These are amplification envelopes, not format-derived
proofs that every legitimate query fits, nor CPU, elapsed-time or RSS guarantees.
A highly amplifying query can still fail explicitly. Formats/operations which did
not have a total record/output quota do not gain one. Metadata and output limits
are separate: repeated serialization of shared metadata is charged independently.

Size admission is **not identity validation**. Files must remain fixed throughout
the operation; readers still check the opened handle/ranges and analyze their fixed
buffers, retaining existing content checks. Growth beyond an admitted read bound
fails. No new content-binding claim is made for ASAR/DEX streams. ASAR may admit a
large carrier without reading it all, and independently checks selected packed or
unpacked content against its opened handle and span limits. JS directories derive
per-file envelopes, not a fixed maximum file count; symlinks are skipped, depth and
worker concurrency remain bounded, and explicit service discovery limits remain.

## Independent guards

- Integer overflow, index/offset validity, canonical identities, original-byte and
  format-width validation are unchanged. Source spans remain 32-bit where required;
  this is not a universal size cap on lazily read archive carriers.
- Recursion, static-fold depth/steps/value expansion, cycle rejection, EVM state /
  stack-cell / value-arena limits and pinned native-engine ABI limits remain.
- PYC marshal object/expanded-object/copy allowances use the selected policy;
  cycles, 128-level recursion, code-object allocation hints and quadratic
  long-to-decimal conversion have separate guards. Fixed-policy library loading
  remains available; no target is imported or executed.
- JVM metadata accepts the original `CONSTANT_Utf8` u16 byte-length range rather
  than an artificial 4096-unit sublimit. Array/parameter/code-length constraints
  and local manifest-inflation/reconstruction/state-expansion guards remain.
- DEX's compatibility `RASC_MAX_INFLATED_ENTRY` still accepts an explicit nonzero
  override. Missing/zero/invalid values select the new CLI expansion envelope;
  MCP does not read this environment override and retains its previous cap.

## Output and failures

.NET, WASM and PYC use a shared fallibly growing JSONL writer; headers, records, summary
and final newline all count. Serialization exhaustion stays `RESOURCE_LIMIT`, not
successful truncation. Existing atomic-output guarantees must remain; this change does not repair DEX's
existing direct file writes or every historical error-classification defect. How much
of an input is validated before answering is a separate decision: .NET now validates
the rows a query reads ([ADR 0017](0017-scoped-dotnet-validation.md)), while the other
backends keep their documented scope.

The ordinary CLI parent spools worker stdout to a private temporary file, forwards
it only after success, and discards it on failure. It no longer retains a second
complete response in parent RAM. Backends can still buffer their complete response;
this is not a claim of fully streaming analysis or a hard process-memory sandbox.

## Validation

Real CLI regressions cover the pinned .NET scan above, a linear EVM scan beyond
one million decoded bytes, a format-valid JVM metadata name over 4096 units, retained PYC depth rejection with empty stdout and preserved
destination, PYC static/source JSON output-limit reporting and complete atomic file
delivery, and unchanged MCP work rejection for the same .NET scan. Small core
checks cover arithmetic overflow and complete JSONL/newline resource classification.
Existing malformed-input, identity, raw-byte and atomic-output regressions remain.

Default all-backend debug/release, focused package/Clippy checks and fresh-process
release measurements are required for this shared runtime change. Results and known
inherited failures must be reported separately; historical measurements under the
old policy are not acceptance evidence for this policy.
