# HTML examples

`rxd html` enumerates lexical records in a local HTML file and extracts exact byte ranges
by content-bound ID. It never executes page scripts, loads external resources or builds a
DOM. Only `entries` and `extract` exist; extracted inline JS/TS is passed to `rxd js`.
`lexical_naive_v1` is a lexical approximation, not a browser parser. See the
[HTML design](../design/html-design.md).

## Setup

Run from the repository root; all outputs go under ignored `target/`. Setup executes no
JavaScript and loads no external resources.

```sh
cargo build
RXD=./target/debug/rxd
DEMO=target/demo/html
mkdir -p "$DEMO"
printf '%s' \
  '<!DOCTYPE html><!--note--><main data-kind="&amp;docs"><a href="/x&amp;y">link</a><script>const answer = 40 + 2; function run(v){return v;}</script></main>' \
  > "$DEMO/page.html"
```

The example is an artificial 154-byte page with an artificial script; it cannot prove
analysis of a real site.

## list

```sh
"$RXD" html entries "$DEMO/page.html"
```

Header backend is `html`, command `entries`, with path/`analysis: lexical`/`parser:
html5gum`/`parser_version: 0.8.4`/`lexical_profile: lexical_naive_v1`. Structured output
follows [ADR 0003](../adr/0003-output-envelope.md), `schema_version=2`. Records appear in
source order: doctype/comment/element/text. IDs are
`rxd1:html:<sha256>:<kind>:<start>:<end>`.

Element detail: tag, ordered attrs, self_closing, void, depth, end_tag,
content_start/content_end. Text/comment/doctype detail: value and depth. Attribute values
are decoded (`/x&amp;y` reports `/x&y`); text and extracted bytes remain original slices.
Summary detail: nodes/elements/texts/comments/doctypes plus tokenizer_errors/
unmatched_end_tags/missing_end_tags. The sample has seven nodes; `--tag script` returns
one element while `summary.detail.nodes` remains seven, distinguishing filtered output
from parse scope.

`list -o FILE` writes the same bytes as stdout. Filters are exact AND combinations:
`--tag` is an ASCII case-insensitive full name; `--attr NAME[=VALUE]` checks presence or
compares the decoded value byte-for-byte (first `=` separates); `--kind` selects one of
element/text/comment/doctype; `--within` validates an element ID and selects its lexical
subtree; `--depth` is relative depth.

```sh
"$RXD" html entries "$DEMO/page.html" --tag script
"$RXD" html entries "$DEMO/page.html" --attr 'href=/x&y'
"$RXD" html entries "$DEMO/page.html" --kind text --within '<MAIN_ID>' --depth 2
```

## extract

```sh
"$RXD" html extract "$DEMO/page.html" \
  --at '<SCRIPT_AT>' \
  --content -o "$DEMO/inline.js"
cat "$DEMO/inline.js"
```

Both `--at` and `-o` are required. `--content` is element-only and selects the content
range (the default outer range otherwise). The file contains the original slice of the
selected half-open container range with an independent SHA-256. Stdout is a single
receipt, not the file content:

```json
{
  "backend": "html",
  "command": "extract",
  "record": "file",
  "schema_version": 2,
  "source_id": "<artifact sha256>",
  "detail": {
    "complete": true,
    "path": "target/demo/html/inline.js",
    "file_bytes": 49,
    "representation": "original",
    "part": "content",
    "origin": {
      "container": "target/demo/html/page.html",
      "format": "html",
      "kind": "element",
      "node_id": "<element id>",
      "part": "content",
      "start": 89,
      "end": 138,
      "source_id": "<container sha256>"
    }
  }
}
```

The receipt `source_id` is the artifact SHA-256; artifact start/end and origin
start/end are strictly separate. Failures (invalid/foreign ID, non-element `--content`,
empty content, output replacing input, budget overflow) emit no payload and leave the
target untouched.

## Continue with JavaScript

```sh
"$RXD" js entities "$DEMO/inline.js"
```

The extracted file is a new source unit; JS offsets refer to its UTF-8 bytes.
`js entities`'s `source_id` matches the artifact receipt. There is no container-to-JS
offset translation; the receipt links the original page to the output. JSON and TS are
selected by the Agent's syntax; arbitrary script content is not promised to parse as JS.

## Budgets and performance

```sh
"$RXD" html entries "$DEMO/page.html" --depth 2 \
  --kind element
"$RXD" html entries "$DEMO/page.html" | \
  "$RXD" html extract "$DEMO/page.html" --from - --content -o "$DEMO/inline.js"
"$RXD" html extract "$DEMO/page.html" --at '<SCRIPT_AT>' --content \
  -o "$DEMO/profile-inline.js" --perf
# The HTML runner was removed; see benches/README.md "Removed per-backend runners".
```

Defaults: input 8 GiB (lowerable; 32-bit spans keep actual addressable content below
4 GiB), no catalog-node limit, no output ceiling (extract is artifact plus receipt), lexical
depth 512. `--perf` stages are read/hash/utf8/tokenize/query/serialize/write. Overflow
returns `RESOURCE_LIMIT` and never truncates or silently skips nodes. Ordinary
representative list/extract follows the 150 ms fresh-process rule: fresh processes,
preserved samples, median/P95, >150 ms share, sizes/hashes, hardware and cache
conditions. The runner written for this backend predates the current command surface
and was removed (see `benches/README.md`).

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, recovery and closed schema are in the
[HTML design](../design/html-design.md#mcp-projection).
