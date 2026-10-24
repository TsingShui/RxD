# ADR 0007: Typed local MCP service and immutable targets

Status: adopted.

## Context

Agents need discoverable typed operations and direct codemode consumption while retaining
CLI identity, ordering, completeness and backend-specific facts. ADR 0003 already defines
structured records; ADR 0002 defines applicable ENUMERATE / SEARCH / XREF / FETCH scope.
The existing one-shot CLI worker boundary protects protocol responsiveness only if MCP
analysis also runs outside the long-lived protocol process.

## Decision

1. Add `rxd mcp serve` behind the optional `mcp` Cargo feature, using exactly pinned
   official Rust SDK rmcp 3.5.1. First release transports newline-delimited JSON-RPC over
   stdio. Support legacy initialization and modern 2026-07-28 per-request discovery.
   Bounded framing, admission, worker drains and complete-response accounting surround
   SDK dispatch. stdout contains protocol messages; diagnostics use stderr.
2. Keep transport, tool service, shared query dispatch and backend analysis separate.
   CLI and MCP use the same backend result construction and explicit result sink.
   Concrete closed input/output schemas describe each operation and record kind.
   Format-specific parsers, IR, identity units and reference semantics remain in backends.
   There is no public arbitrary argv, shell or caller-selected output destination.
3. Convert ordinary JSONL to `{records: [...]}`, retaining fixed nullable navigation
   fields, ordering and record details. Recursive JS stays flat, with each file's rows
   opened by a `file` data record. Native single JSON remains a single object.
   FETCH retains exact source or original instruction evidence and existing reconstruction
   fallback. JS isolated UTF-16 surrogate values use explicit `utf16_units` objects rather
   than replacement characters. Both structuredContent and its JSON compatibility text
   copy count towards the full response budget. Successful results are never silently cut.
4. Accept exactly one explicit path-or-target input reference per normal analysis call.
   One-shot paths are bounded captures, without durable handles. `rxd_open` makes immutable
   private copies and returns a process-local opaque target handle; `rxd_close` invalidates
   future acquisitions. Content-bound entity IDs still select backend units and are checked
   against the captured bytes. Acquisition retains storage and quota through worker reaping,
   even after close/expiry. Server roots apply to primary inputs and declared dependencies.
5. JS directories have a deterministic bounded file-set snapshot. ASAR open/extract captures
   declared unpacked dependencies; metadata-only direct list need not read those dependencies.
   Existing ASAR links remain unsupported. DEX bundles preserve order and duplicate semantics.
   ASAR extraction publishes a separately retained child target and provenance, with opt-in
   base64 bytes. Targets do not cache mutable external reads or create implicit current inputs.
6. Every analysis, including DEX and linked Kuna, runs in a disposable isolated worker.
   Request cancellation, deadlines and client EOF stop and reap associated workers.
   Admission fails immediately when configured slots are occupied; no unbounded queue.
   Worker/thread totals, request frames, IPC, snapshots and complete responses have finite
   budgets. Linked native workers additionally have an owned-process sampled RSS guard
   (nominal 5 ms, default/maximum 256 MiB, lowering only), with stop/reap and explicit error
   on observed excess; this is not an instantaneous kernel cap. Worker isolation is fault containment, not a general OS execution sandbox.
7. Discovery describes actual backend capabilities and optional engine availability.
   DEX null entity IDs and EVM missing SEARCH/XREF/FETCH closure remain visible.
   Optional Kuna MCP coverage is only native JSON function enumeration and single-function
   decompilation; it retains the pinned upstream JSON contracts, without inventing rxd IDs.
   Linked Kuna's one-shot global setup remains confined to workers. No upstream source edits.
8. Distributed single-file builds include `mcp,bundled-kuna,bundled-ilspy` with a
   native producer. Default source builds include MCP and all backend namespaces but
   no desktop native engine; engine initialization stays lazy. Four-platform CI runs
   public-process release contracts and relocated smoke; passing cross-compilation
   is not runtime evidence.

## Consequences and acceptance

The single test seam is a real stdio client → actual rxd process.
Reuse CLI fixtures and independent oracles; compare equivalent CLI/MCP records, original
bytes, returned-ID workflows and failures. Also test public protocol lifecycle, cancellation,
EOF, overflow, worker failure and snapshot races. Ordinary E2E contains no latency assertions.

Release evidence separately reports fresh CLI, server startup/initialize/first path query,
open, repeated same-target query, returned-ID complete workflows, and concurrency/RSS.
Fresh-session boundaries include complete `tools/list` typed capability discovery and
routing; full fresh navigation includes required target lifecycle and actual returned IDs.
Keep startup/init and resident query components distinct from those complete fresh totals.
Freeze executables and build/source/engine identities, keep at least 20 samples per workload,
retain failed and timed-out samples, and include complete writes. Production and synthetic
inputs stay separately identified. No pooled acceptance or implicit decompilation exception.
Existing exact numeric approvals and EVM's explicit capability-first scope stay unchanged.

Future HTTPS can add Streamable HTTP around the same service/query/worker boundary.
Remote authentication, authorization, per-caller targets/quotas, TLS and uploads require a
separate milestone; local path/handle semantics cannot simply be exposed to the network.
