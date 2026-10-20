# JVM method queries

```sh
cargo build --release
target/release/rxd jar methods tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture -o target/jvm-methods.jsonl
```

Structured output follows [ADR 0003](../adr/0003-output-envelope.md),
`schema_version=2`: streaming commands emit header → records → summary, navigation
outside and content under `detail`. Take a `methods` row's `at` (`Lx;->name(proto)`) and
query:

```sh
target/release/rxd jar methods tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;->add(II)I'
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;->add(II)I' --raw
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;->add(II)I' -o target/add.java
# locate → fetch without copying a coordinate: pipe the whole stream, header included,
# so the fetch can refuse a stream that describes another archive
target/release/rxd jar methods tests/fixtures/jvm/flow-deflated.jar | \
  target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar --from -
```

`get-method` default Java and `get-class` Java are outside the envelope. `--raw`
returns one `record="bytecode"` JSON object; instructions are in `detail.instructions`
and the original Code range is outer start/end.

Whole-class decompilation accepts internal, dotted or descriptor class names and emits a
single Java source file:

```sh
target/release/rxd jar get-class tests/fixtures/jvm/flow-deflated.jar --at fixture.FlowFixture
target/release/rxd jar get-class tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;' -o target/FlowFixture.java
```

List constant-pool string literals (`--class`/`--filter` narrow the scope):

```sh
target/release/rxd jar strings tests/fixtures/jvm/flow-deflated.jar
target/release/rxd jar strings tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture --filter hello
```

Search cross-class references (string/type/method/field; method/field may use `--class`
owner substring):

```sh
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar -f toJson --kind method
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar -f JsonReader --kind type
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar -f append --kind method --class java/lang/StringBuilder
target/release/rxd jar xref tests/fixtures/jvm/flow-deflated.jar -f hello --kind string
# An identity, exactly: one method overload, as a `xref` row prints it.
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar --at 'Lcom/google/gson/Gson;->toJson(Ljava/lang/Object;)Ljava/lang/String;' 
```

Runnable returned-ID routing:

```sh
python3 - <<'PY'
import json, subprocess
binary = 'target/release/rxd'
path = 'tests/fixtures/jvm/flow-deflated.jar'
rows = map(json.loads, subprocess.check_output([binary, 'jar', 'methods', path], text=True).splitlines())
method = next(row for row in rows if row.get('record') == 'method' and '->add(' in row['at'])
subprocess.run([binary, 'jar', 'get-method', path, '--at', method['at']], check=True)
PY
```

No Java or D8 is needed. `--release 17` selects the matching class version in a
multi-release JAR. Dynamic-instruction methods can be inspected with `--raw`;
unsupported Java reconstruction returns an explicit error. See the
[JVM design](../design/jvm-design.md).

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend IDs, offsets, inference and degradation are in the
[JVM design](../design/jvm-design.md#mcp-projection).
