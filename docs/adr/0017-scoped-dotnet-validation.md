# ADR 0017: Validate the rows a query reads, not the whole module

Status: accepted. Applies to `rxd dotnet`; other backends keep their own scope rules.

## Context

The .NET backend used to validate the entire module before answering anything:
cross-table/heap/token structure, every method and field signature grammar, every
`MemberRef`/`MethodSpec`, and every property/event association. Any corrupt row — even
one unrelated to the question — aborted the command with `INVALID_INPUT`.

That coupled local correctness to whole-file integrity. Fetching one method from a
17 MB assembly paid for signatures of ~30k methods and a 15 ms reference-scanner build,
and a single damaged row anywhere made every listing, xref and FETCH fail.

## Decision

A query validates exactly what it reads.

- PE/metadata headers are parsed up front; each table row, heap entry and blob is
  bounds/dialect checked at the point of use.
- A listing resolves signatures only for the rows it emits; `get-method` resolves the
  selected method and the tokens its body uses; `get-class` resolves its type and
  methods; `xref` resolves the members it actually matches and the bodies it scans.
- `MemberRef`/`MethodSpec` grammar and property/event associations are resolved on use.
- A query that needs a corrupt row still fails with `INVALID_INPUT`; corrupt rows
  outside that set no longer fail it.
- A metadata row that cannot be indexed at all (invalid owner, nesting or generic
  token) is recorded internally and skipped. Queries that do not need it answer
  normally; a query that names it fails with its reason. Resource-limit exhaustion is
  never downgraded to a skipped row.
- Producing the requested unit is the success criterion. `complete` keeps its
  stream-delivery meaning and stays true for a successful .NET stream; the backend does
  not add per-row caveats to the envelope.
- `Input::verify_module` retains the full cross-table/heap/token pass as an explicit
  opt-in for callers that want a whole-module report. It is not part of a query.
- MCP keeps its independent fixed service quotas (capture, concurrency, deadline,
  response/projection), unchanged by this decision.

`complete` keeps its stream-delivery meaning: the records are all the records the
query produced for its scope. It no longer implies that unread rows were validated.

## Consequences

- Behaviour change: a corrupt unselected signature/reference no longer aborts a local
  query. Tests that encoded the old whole-file rule were replaced with scoped
  expectations, and `verify_module` covers the explicit full check.
- Measured on the locally unpacked .NET 10.0.12 `System.Private.CoreLib` (17,088,296
  bytes), fresh CLI processes, 20 samples after two warmups, wall clock median/P95:
  `get-method --raw` 59.5/62.3 → 43.3/47.7 ms; `methods` 62.2/65.4 → 42.7/45.0 ms;
  `classes` 59.4/61.9 → 42.2/44.5 ms. Newtonsoft 712 KB `get-method` 16.4 → 14.2 ms.
  `xref` over a whole module is unchanged in kind (it scans every body and resolves
  each body's signature), so its cost stays proportional to the scan.
- A successful .NET stream no longer certifies whole-module integrity. Agents that need
  that must call `verify_module`; the CLI does not expose a flag for it yet.
- Remaining floor on the 17 MB input is read+SHA-256 (~8 ms), header/row parsing and
  names/owners (~12 ms), and process/worker startup (~10–15 ms).
- Other backends are untouched: DEX/JVM/WASM/PYC/JS/HTML/EVM keep their documented
  behaviour and tests until an equivalent decision is made for them.
