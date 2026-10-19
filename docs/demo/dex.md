# DEX CLI examples

Run `cargo build --release`. `INPUT` is an authorized APK, DEX or ZIP with a root DEX;
`CLASS` is a real name from `classes` output. Commands analyze statically and never
install or run the application.

```sh
target/release/rxd dex entries /path/to/app.apk
target/release/rxd dex classes /path/to/app.apk -f example
target/release/rxd dex strings /path/to/app.apk --filter Authorization --limit 20
target/release/rxd dex strings /path/to/app.apk --filter Authorization --offset 20 --limit 20
target/release/rxd dex xref /path/to/app.apk string Authorization
target/release/rxd dex xref /path/to/app.apk type Gson
target/release/rxd dex xref /path/to/app.apk method onCreate --class com.example.Main
target/release/rxd dex xref /path/to/app.apk field INSTANCE --class example --fuzzy-class
target/release/rxd dex get-class /path/to/app.apk com.example.Main
target/release/rxd dex get-class /path/to/app.apk 'Lcom/example/Main;' --members -o target/Main.java
target/release/rxd dex manifest /path/to/app.apk -o target/AndroidManifest.xml
target/release/rxd dex fields-plan --apk /path/to/app.apk --descriptor com.example.Main
target/release/rxd dex member-by-index --apk /path/to/app.apk --descriptor com.example.Main --method-index 339
target/release/rxd dex member-by-index --apk /path/to/app.apk --descriptor com.example.Main --field-index 3
target/release/rxd dex classes /path/to/classes.dex --threads 2
```

The shared Agent skill is installed with the root entry and embedded in the binary:

```sh
target/release/rxd skill install --print
target/release/rxd skill install --dir target/skills
```

The second writes `target/skills/rxd/SKILL.md`; see the
[installation guide](../../README.md#-installation) for user Agent directories.

`-o` writes the same bytes while still emitting stdout; diagnostics and `--debug` go to
stderr. Multi-input index commands may repeat `--apk` or supply multiple files. Method
indices, field_ids and field slots use the actual input's indices. Exit 3 means no index
result and 4 means ambiguity; `member-by-index` still returns candidate JSON at 4.

```sh
RASC_MAX_INFLATED_ENTRY=1048576 target/release/rxd dex manifest /path/to/app.apk
```

See the [DEX design](../design/dex-design.md) for the full contract and resource limits.
The old rasc MCP transport is not imported; the shared stdio entry is in the
[MCP examples](mcp.md).

## Consuming schema v2

`classes/strings/xref/entries/member-by-index` return header → records → summary
JSONL even for zero results; success requires exit 0 and `summary.complete=true`.
Navigation fields are always present but null; FETCH IDs are never constructed from
indices. Route from `xref.detail.class` or `classes.detail.descriptor` to
`get-class`:

```sh
target/release/rxd dex xref /path/to/app.apk string Authorization > target/dex-xref.jsonl
jq -r 'select(.record == "reference") | .detail.class' target/dex-xref.jsonl
target/release/rxd dex get-class /path/to/app.apk 'Lcom/example/Main;' > target/Main.java
```

`strings.detail.value/index` use the global DEX string table; `detail.class` is always
null because a string need not belong to a class. `fields-plan` is one
`record=field_plan` envelope with `backend=dex`, `command=fields-plan`,
`schema_version=2`; read layout from `detail.instance_fields` etc. Exits 3/4 emit no
layout. The old rasc pipe-delimited output and `fields-plan/v1` label are removed.
`get-class --members` comment tables, Java text and Manifest XML remain unchanged.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[DEX design](../design/dex-design.md#mcp-projection).
