# ASAR backend

Status: implemented.

`rxd asar` is the Agent-facing static entry point for ASAR containers. It provides
`entries` and `extract`. `rxd-asar` depends only on shared budgets, protocols and file
reading; it does not depend on Oxc or `rxd-js` and does not classify content by file
extension. Extracted JavaScript is analyzed by `rxd js`. See the
[ASAR examples](../demo/asar.md).

## Inputs and identity

The input is a regular ASAR file validated by content, not an application directory,
installer, ZIP, or executable app entry. The Agent locates the actual ASAR inside a
release or a prepared application directory and passes it here. Installer handling
belongs to a separate isolated preparation step; this backend never installs, runs,
or self-extracts a sample.

## Commands

| Command | Output and behavior |
| --- | --- |
| `asar entries FILE [-o CATALOG] [-f FILTER]` | header/file/summary JSONL, `backend: asar`, `command: entries`; each file row carries `at` (archive-relative path); `header.detail.analysis: metadata`. Lists every file type ordered by logical path. `-o` writes the same bytes as stdout. |
| `asar extract FILE --at ENTRY -o OUTPUT` | `--at` (an `entries` row) and `-o` are required. The target contains the original bytes; stdout is a single JSON receipt, never the file content. |

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`. `entries` places format-specific fields in each file record and
the summary under `detail`; `extract` returns one `file` record with
`backend`/`command`/`schema_version`/`source_id` outside and the receipt in `detail`.

A `entries` file record has `path`, `bytes` and `storage`. Packed entries expose
`range_valid`; invalid ranges expose
`range_error: out_of_bounds|overflow`. Link entries expose `link`.
`summary.detail.invalid_ranges` counts invalid ranges. Unpacked readability, syntax,
decryption and signatures are outside metadata enumeration. ASAR links are listed,
not followed. `complete` means the index was fully enumerated; it does not prove every
file is readable. Range status uses the container length at open time.

An `extract` receipt has `source_id` (SHA-256 of the extracted file bytes) and
`detail` with `complete`, output `path`, `file_bytes`, `storage`,
`representation: original` and `origin`. `origin` contains the container path,
`format: asar`, `entry` and `header_id`; `header_id` hashes the header Pickle and is
not a whole-archive integrity claim. The caller chooses the output path; host paths
are never derived from untrusted entry names. Neither the existing archive nor the
selected unpacked input may be the output target.

## Safety and read boundary

Only a bounded header and the requested entry are read. The backend never unpacks
everything, executes input code, or loads native modules. The index structure and
budgets are validated at open; unrelated packed entries that are out of bounds do not
fail the whole operation. Before allocation and seek, the target range
`data_start + offset + size` is checked against the actual length of the same open
handle, including zero-length entries. Buffers are allocated only after byte and
read limits pass. Unpacked files come from the adjacent `<archive>.unpacked` tree;
Unix traversal uses no-follow directory handles layer by layer and verifies the
actual length against the header.

Duplicate paths, invalid path components, corrupt Pickle/JSON, invalid decimal
offsets, values not representable as `u64`, and budget violations are rejected
globally. A selected entry with an out-of-range or overflowing range returns
`INVALID_INPUT` without creating a file or success receipt. Output replacement is
atomic only after target reads, hashing and receipt budgets pass; earlier failures
preserve the existing `-o` target. If stdout fails after replacement, the output file
may exist and a non-zero exit still means the operation was not fully successful.
Multi-file extraction and directory transactions are not implemented.

## Resource budgets

Defaults: header budget 8 MiB; the shared budget 10,000 (including
directories); paths at most 4,096 UTF-8 bytes; serde_json's default recursion guard.
the shared budget defaults to 8 GiB and may be lowered to between 1 byte and 8 GiB;
the selected original file is charged, not a declared whole-archive size. 32-bit
spans keep actual addressable content below 4 GiB. There is no record or output
ceiling: `entries` prints every entry and `extract` writes the whole artifact plus its
receipt, because a narrower answer is the caller's filter, not a budget. Parsing keeps
its input, entry and work ceilings, and a real overflow there is an atomic
`RESOURCE_LIMIT` before any file is replaced. No silent truncation or skipped target.

## Relationship to JavaScript

Extracted `.js`/`.cjs`/`.mjs`/`.jsx`/`.ts`/`.tsx` content is analyzed by `rxd-js`;
original bytes and content-bound IDs under the same parse options are preserved. JS
offsets refer to the extracted file, and the extraction receipt retains the origin
relationship. Native `.node` files, shared libraries, V8 compile artifacts and other
binaries can be enumerated and extracted but are never executed or interpreted here.
IPC, contextBridge, preload/renderer relationships are not implemented. JS
deobfuscation and string decryption remain permanent non-goals.

## Performance diagnostics

`--perf` marks analysis and CLI records with `backend: asar` and command `entries` or
`extract`. `container` handles the header, `read/hash` the selected raw bytes,
`serialize` the catalog or receipt, and `write` the atomic file plus receipt output.
For `extract`, `analysis.output_bytes` is file plus receipt and `CLI.output_bytes` is
the stdout receipt; `list -o` reports zero CLI output bytes. `entries` has no single
`source_id`; `extract` uses the selected content identity. A worker is not an OS
security sandbox.

The ASAR runner measured ordinary ASARs and an ASAR with 100 fake 1 GiB entries,
using fixed production JavaScript and explicitly artificial binary bytes. It covers
`entries`, `extract` and `extract → js find` with fresh CLI/worker processes, real atomic
writes and per-workload samples, median, P95 and >150 ms share. These fixtures are not
real protected application packages.

## MCP projection

`asar_entries`/`asar_extract` reuse the same index and extraction implementation.
`entries` projects header/records/summary without reading unrelated unpacked
dependencies. `open`/`extract` capture declared dependencies and validate roots and
lengths; links are not followed. `extract` keeps the original receipt with
`detail.path: null` and adds `child_target`; `include_bytes` adds `bytes_base64` but
does not copy bytes by default. The child target owns the extracted bytes and is
independent of the parent target's lifetime; `origin` retains the container and entry.
No caller-supplied host write path is used.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
