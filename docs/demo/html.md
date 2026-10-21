# HTML examples

`rxd html` enumerates lexical records in a local HTML file and extracts exact byte ranges
by content-bound ID. It never executes page scripts, loads external resources or builds a
DOM. `entries` lists lexical nodes; `resources` classifies resource candidates and can
include inline bodies; `extract` fetches one original range. JS content is passed to
`rxd js` with its own source identity.
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

## Resource discovery and batch inline handoff

This local example performs no requests. The document URL is explicit; an HTTP
capture should supply its final URL after redirects.

```sh
set -eu
printf '%s' \
  '<base href="/assets/"><script src="app.js" defer></script>' \
  '<link rel="stylesheet" href="app.css"><link rel="preconnect" href="https://cdn.example.org">' \
  '<script type="application/json">{"notJavaScript":true}</script>' \
  '<script type="module">export const answer = 42;</script>' \
  '<script>const label = "λ";</script>' > "$DEMO/resources.html"

"$RXD" html resources "$DEMO/resources.html" \
  --base-url 'https://example.org/account/start' -o "$DEMO/resources.jsonl"
"$RXD" html resources "$DEMO/resources.html" \
  --kind script --location inline --include-content -o "$DEMO/inline-bodies.jsonl"
```

The URLs for app.js/app.css resolve under `https://example.org/assets/`. The
preconnect is `kind: hint`, not an asset to download; application/json is `data`,
not JS. `detail.conditions` retains declared loading conditions, not their runtime
truth. Relative references without a base, srcset and unsupported external import
maps stay unresolved. Filter by kind/location/name without changing source identity.

The next step writes trusted, hash-derived filenames from one result stream, not
one CLI extraction call per inline script. It never evaluates the returned text.

```sh
python3 - "$DEMO/inline-bodies.jsonl" "$DEMO/inline-batch" <<'PY'
import hashlib, json, pathlib, sys
rows = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text().splitlines()]
if rows[-1].get("complete") is not True:
    raise SystemExit("incomplete catalog")
out = pathlib.Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
manifest = []
for row in rows[1:-1]:
    detail = row["detail"]
    body = detail["content"].encode("utf-8")
    source_id = hashlib.sha256(body).hexdigest()
    if source_id != detail["content_source_id"]:
        raise SystemExit("inline content identity mismatch")
    extension = ".mjs" if detail["script_type"] == "module" else ".cjs"
    artifact = out / (source_id + extension)
    artifact.write_bytes(body)
    manifest.append({
        "path": str(artifact), "source_id": source_id,
        "origin": {"source_id": rows[0]["source_id"], "at": row["at"],
                   "start": detail["content_start"], "end": detail["content_end"]},
    })
(out / "origin.json").write_text(json.dumps(manifest, indent=2))
PY
"$RXD" js entities "$DEMO/inline-batch" --depth 0 --threads 2
```

Both scripts parse without execution; the JSON data block is excluded. The JSON
manifest is not a JS scan input. JS offsets are artifact-relative, and its header
identity matches `content_source_id`, not the HTML container identity.

For one body, the resource coordinate also works with ordinary extraction:

```sh
"$RXD" html resources "$DEMO/resources.html" --kind script --location inline \
  -o "$DEMO/inline-nodes.jsonl"
"$RXD" html extract "$DEMO/resources.html" --from "$DEMO/inline-nodes.jsonl" \
  --content -o "$DEMO/one-inline.mjs"
```

`--from` checks the catalog's SHA-256 against the very source buffer used for
extraction, including same-length changed inputs. No HTML-to-JS coordinate guessing
or synthetic resource ID is needed. A complete catalog is not browser page coverage.

## extract

```sh
"$RXD" html extract "$DEMO/page.html" \
  --at '<SCRIPT_AT>' \
  --content -o "$DEMO/inline.js"
cat "$DEMO/inline.js"
```

One of `--at`/`--from` and `-o` are required. `--content` is element-only and selects the content
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
