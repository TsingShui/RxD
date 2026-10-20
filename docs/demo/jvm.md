# JVM method queries

```sh
cargo build --release
target/release/rxd jar methods tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture -o target/jvm-methods.jsonl
```

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`: streaming commands emit header → records → summary, navigation
outside and content under `detail`. Take the method record's outer `id` (the name is in
`detail.name`) and query:

```sh
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar '<method-id>'
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar '<method-id>' --bytecode
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar '<method-id>' -o target/method.java
```

`get-method` default Java and `get-class` Java are outside the envelope. `--bytecode`
returns one `record="bytecode"` JSON object; instructions are in `detail.instructions`
and the original Code range is outer start/end.

Whole-class decompilation accepts internal, dotted or descriptor class names and emits a
single Java source file:

```sh
target/release/rxd jar get-class tests/fixtures/jvm/flow-deflated.jar fixture.FlowFixture
target/release/rxd jar get-class tests/fixtures/jvm/flow-deflated.jar 'Lfixture/FlowFixture;' -o target/FlowFixture.java
```

List constant-pool string literals (`--class`/`--filter` narrow the scope):

```sh
target/release/rxd jar strings tests/fixtures/jvm/flow-deflated.jar
target/release/rxd jar strings tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture --filter hello
```

Search cross-class references (string/type/method/field; method/field may use `--class`
owner substring):

```sh
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar method toJson
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar type JsonReader
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar method append --class StringBuilder
target/release/rxd jar xref tests/fixtures/jvm/flow-deflated.jar string hello
```

Runnable returned-ID routing:

```sh
python3 - <<'PY'
import json, subprocess
binary = 'target/release/rxd'
path = 'tests/fixtures/jvm/flow-deflated.jar'
rows = map(json.loads, subprocess.check_output([binary, 'jar', 'outline', path], text=True).splitlines())
method = next(row for row in rows if row.get('record') == 'method' and row['detail']['name'] == 'add')
subprocess.run([binary, 'jar', 'get-method', path, method['id']], check=True)
PY
```

No Java or D8 is needed. `--release 17` selects the matching class version in a
multi-release JAR. Dynamic-instruction methods can be inspected with `--bytecode`;
unsupported Java reconstruction returns an explicit error. See the
[JVM design](../design/jvm-design.md).

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[JVM design](../design/jvm-design.md#mcp-projection).
