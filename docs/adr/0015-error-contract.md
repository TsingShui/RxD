# ADR 0015: One failure, one code, one exit status

Status: accepted. Applies to every CLI command and to the MCP projection.

## Context

The same two failures produced three answers, and one backend produced none:

| condition | JS | WASM | DEX | JVM | .NET |
| --- | --- | --- | --- | --- | --- |
| malformed coordinate | `INVALID_ID` / 1 | `INVALID_ID` / 1 | no code / 1 | `INVALID_QUERY` / 2 | `INVALID_QUERY` / 2 |
| well formed, names nothing | `INVALID_ID` / 1 | `INVALID_QUERY` / 2 | no code / 1 | `INVALID_QUERY` / 2 | `INVALID_QUERY` / 2 |
| corrupt input | `INVALID_INPUT` / 1 | — | no code / 1 | `INVALID_INPUT` / 1 | `INVALID_INPUT` / 1 |

Three causes:

1. **The code lived in the message.** Backends wrote `bail!("INVALID_QUERY: ...")` and
   `.with_context(|| format!("INVALID_QUERY: ..."))`; the CLI then re-applied a code it
   had guessed, so diagnostics read `INVALID_QUERY: INVALID_INPUT: ...`.
2. **Each CLI adapter guessed the code from the message text** with its own
   `message.contains("RESOURCE_LIMIT")` list, falling back to `IO_ERROR`. Four adapters,
   four lists, and a code word inside a sentence could pick a wrong class.
3. **DEX bypassed the typed path**: `execute() -> i32` printed `Error: ...` and exited 1,
   so no DEX failure ever carried a code.

## Decision

### 1. One failure carries exactly one code, as data

`rxd_core::Error { code, message }` is the carrier, and `rxd_core::Failure` is the closed
set. A code never appears in a message; a message never decides a code.

### 2. The set is grouped by what the caller can do

| group | code | meaning | caller's move |
| --- | --- | --- | --- |
| fix the query | `INVALID_QUERY` | malformed query or contradictory arguments | correct the query |
| | `NOT_FOUND` | well formed, names no record | re-read a listing |
| | `AMBIGUOUS` | a name matches several identities | use a coordinate |
| change the input | `INVALID_INPUT` | bytes are corrupt or are not this format | different input |
| | `UNSUPPORTED_INPUT` | well formed, not implemented here | different approach |
| | `SOURCE_MISMATCH` | a coordinate or stream came from another input | re-run the listing |
| | `RESOURCE_LIMIT` | a declared budget was reached | narrow the query |
| | `DECOMPILE_ERROR` | the input parsed and source recovery failed | read the original |
| environment | `IO_ERROR` | file, pipe or device failure | retry |
| | `INTERNAL_ERROR` | a broken invariant, including a dead worker | report a bug |

`INVALID_ID` and `FORMAT_ERROR` are retired: an identifier is a coordinate now, and a
format error is an input error. Both stay readable in `Failure::from_code` so output from
an older release still classifies.

### 3. The exit status is a property of the class

`0` success, `2` a query the caller can correct (`INVALID_QUERY`, `NOT_FOUND`,
`AMBIGUOUS`), `1` everything else. `Error::exit_code()` is the only implementation, and
both process paths (bypass and analysis worker) use it.

### 4. One boundary decides, and only it

`cli::classify` turns a backend failure into a coded one: an error that already carries a
code keeps it, a device failure is `IO_ERROR`, a leading known code inside a context
string is honoured once and stripped, and anything else is `INVALID_INPUT` - what a
parser produces when the bytes do not make sense. Backends override that default with
`Error::failure(...)` whenever they know better. The four per-adapter code lists are gone.

### 5. Every backend keeps these three promises

- A coordinate that does not parse is `INVALID_QUERY`.
- A coordinate that parses and names nothing is `NOT_FOUND`.
- A backend reports what happened, never how to label it: no code prefixes in messages.

## Consequences

- The same two probes now answer `INVALID_QUERY` / 2 and `NOT_FOUND` / 2 on every backend,
  `SOURCE_MISMATCH` / 1 for a stream from another input, `IO_ERROR` / 1 for an unreadable
  file, and `INVALID_INPUT` / 1 for corrupt bytes.
- "Exit 2" now means "your query was wrong", so a script can retry the query instead of
  the whole run; a coordinate that names nothing is included because re-running the
  listing fixes it.
- The message may still begin with a code while older backends spell one into a context;
  the boundary strips it, so a diagnostic never repeats a code. New code should construct
  `Error::failure` instead.
