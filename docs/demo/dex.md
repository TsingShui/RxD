# DEX CLI examples

Run `cargo build --release`. `INPUT` is an authorized APK, DEX or ZIP with a root DEX;
`CLASS` is a real name from `classes` output. Commands analyze statically and never
install or run the application.

```sh
target/release/rxd dex entries /path/to/app.apk
target/release/rxd dex classes /path/to/app.apk -f example
target/release/rxd dex strings /path/to/app.apk --filter Authorization --limit 20
target/release/rxd dex strings /path/to/app.apk --filter Authorization --offset 20 --limit 20
target/release/rxd dex xref /path/to/app.apk -f Authorization --kind string
target/release/rxd dex xref /path/to/app.apk -f Gson --kind type
target/release/rxd dex xref /path/to/app.apk -f onCreate --kind method --class com.example.Main
target/release/rxd dex xref /path/to/app.apk -f INSTANCE --kind field --class example
# An identity, exactly: every call site of one overload.
target/release/rxd dex xref /path/to/app.apk --at 'Lcom/example/Main;->onCreate(Landroid/os/Bundle;)V'
# A literal, exactly (the one anchor `--kind` has to name).
target/release/rxd dex xref /path/to/app.apk --at Authorization --kind string
target/release/rxd dex get-class /path/to/app.apk --at com.example.Main
target/release/rxd dex manifest /path/to/app.apk -o target/AndroidManifest.xml
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

```sh
RASC_MAX_INFLATED_ENTRY=1048576 target/release/rxd dex manifest /path/to/app.apk
```

See the [DEX design](../design/dex-design.md) for the full contract and resource limits.
The old rasc MCP transport is not imported; the shared stdio entry is in the
[MCP examples](mcp.md).

## Consuming schema v2

JSONL even for zero results; success requires exit 0 and `summary.complete=true`.
Navigation fields are always present but null; FETCH coordinates are never constructed
from indices. Route from `classes` rows (their `at` is `Lx/y;`) or from a `methods` row's
`context_at`/`at` into `get-class`/`get-method`:

```sh
target/release/rxd dex xref /path/to/app.apk -f Authorization --kind string > target/dex-xref.jsonl
jq -r 'select(.record == "reference") | .context_at' target/dex-xref.jsonl
target/release/rxd dex get-class /path/to/app.apk --at 'Lcom/example/Main;' > target/Main.java
```

`strings.detail.value/index` use the global DEX string table; `detail.class` is always
`schema_version=2`; read layout from `detail.instance_fields` etc. Exits 3/4 emit no

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[DEX design](../design/dex-design.md#mcp-projection).
