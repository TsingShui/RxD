# ADR 0002: Backend capability closure

Status: accepted as the acceptance standard for all analysis backends.

This ADR defines when an analysis backend is "complete": it must let an Agent complete
the reverse-engineering loop instead of exposing isolated commands. It applies to all
implemented and future backends (JS, ASAR, HTML, JVM, DEX, .NET, WASM, EVM, PYC, and
so on).

Every backend design document must reference this ADR and explain its concrete commands
and any exemption in that backend's context.

## Context

Real reverse engineering follows **locate → relate → read**: an Agent starts from a
clue (usually a string, symbol or structure inventory), finds who references it, then
reads the relevant code unit. A backend that can list structures and fetch a method but
cannot search references breaks the chain and prevents independent completion. Backends
had accumulated commands without a shared definition of completeness.

## Decision

A backend has four capability classes. Applicability is determined by the **nature of
the format**; an applicable class is mandatory, and an inapplicable class needs an
explicit exemption in the backend design.

### 1. ENUMERATE

List the format's top-level structures and assign each a **content-bound ID**. This is
the navigation entry point; without usable IDs, later FETCH cannot locate anything.
Examples: JS `entities`, JVM `classes`, DEX `classes`, .NET `classes`, WASM
`entities`, ASAR `entries`.

### 2. SEARCH

Locate clues by string/constant literals (hardcoded URLs, keys, error messages).
Formats with readable literals are applicable; pure binary containers may justify an
exemption. Examples: JS `find`, JVM/DEX/.NET/WASM `strings`.

### 3. XREF

From a symbol, string, type or member, find who references it. This is the "relate"
step; without it the loop is broken. Formats with code instructions or reference
relationships are applicable. Examples: JS `xref`, DEX `xref`, .NET `xref`, WASM
`xref`.

### 4. FETCH

Read a unit by ID or name. Granularity follows the format:

- **Unit-level is always required**: one method/function/file. Examples: JS `get`,
  JVM `get-method`, .NET `get-method`, WASM `get-function`, ASAR `extract`.
- **Aggregate-level is required only when the format naturally has aggregate
  structure**: whole class/module. C#/JVM have classes, so `get-class` is required.
  WebAssembly and ELF have no class aggregate, so aggregate FETCH is exempt (module
  ENUMERATE is enough). JS aggregate semantics are defined in its design document.

## Closure test

- ENUMERATE IDs can be passed directly to FETCH.
- SEARCH and XREF hits carry their owning unit (class/function) so an Agent can route
  to FETCH.
- All applicable classes together, with working routing, form a complete loop.

## Applicability

- **Code formats** (JVM, DEX, .NET, WASM, native, PYC): normally all four apply;
  aggregate FETCH follows the rule above.
- **Container formats** (ASAR): enumerate plus unit FETCH are central; SEARCH/XREF on
  the container itself are exempt because contained files belong to the corresponding
  code backend.
- Every exemption must be stated in the backend design; silent omission is not allowed.

## Current status

| Capability | JS | JVM | DEX | .NET | WASM | ASAR | HTML | PYC | EVM |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ENUMERATE | entities | classes | classes | classes | functions | entries | entries | codes | blocks/interface |
| SEARCH | find | strings | strings | strings | strings | exempt (container) | `--attr` (limited metadata) | strings | strings (PUSH/data candidates) |
| XREF | xref | xref | xref | xref | xref | exempt (container) | exempt (no code-reference semantics) | xref (explicit name/constant) | xref (literal/CODECOPY/control flow) |
| FETCH unit | get | get-method | get-method (unit selection available) | get-method | get-function | extract | extract | get-code (raw instructions) | get-function/get-block |
| FETCH aggregate | see JS design | get-class | get-class | get-class | exempt (no classes) | n/a | n/a (no aggregate) | decompile (whole module; source completeness separate) | exempt (no source classes; selected shared regions travel with function) |

DEX's traditional whole-class decompilation makes `get-class` the practical unit
selection; there is no separate `get-method`. HTML is a lexical container: enumerate
plus node extraction, with attribute matching as limited metadata search and an
explicit exemption from general text search/code XREF. EVM has no source-level class
aggregate and keeps selected shared regions with the function.

## Consequences

- New backends are reviewed against the four-class loop; gaps are recorded as pending
  or justified exemptions.
- Command naming follows existing conventions: unit fetches use
  `get-method`/`get-function`, aggregate fetches use `get-class`, avoiding an ambiguous
  `get`.
- Performance contracts are unchanged: ordinary queries follow the 150 ms requirement;
  whole-function/class/module decompilation exceptions follow
  [AGENTS.md](../../AGENTS.md#performance-requirements) and are scoped individually.
