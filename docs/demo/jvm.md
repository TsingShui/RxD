# JVM method queries

Ordinary CLI archive/index/entry/work envelopes scale with admitted input. JVM
format widths, code length and reconstruction-state guards remain; valid metadata
names are not capped at 4096 units.
MCP service quotas remain independent and unchanged.

```sh
cargo build --release
target/release/rxd jar methods tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture -o target/jvm-methods.jsonl
```

Structured output uses `schema_version=2`: streaming commands emit data rows, navigation
outside and content under `detail`. Take a `methods` row's `at` (`Lx;->name(proto)`) and
query:

```sh
target/release/rxd jar methods tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;->add(II)I'
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;->add(II)I' --raw
target/release/rxd jar get-method tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;->add(II)I' -o target/add.json
jq -r '.detail.code' target/add.json
```

Every FETCH is one JSON record: `get-method` and `get-class` answer with
`record="method_body"` / `record="class_body"`, the Java source in `detail.code`,
`detail.representation="java"` and `detail.decompiled=true`. `--raw` keeps the same
record and switches `detail.representation` to `bytecode`, with instructions in
`detail.instructions` and the original Code range in outer start/end.

Whole-class decompilation accepts internal, dotted or descriptor class names and returns
one `class_body` record:

```sh
target/release/rxd jar get-class tests/fixtures/jvm/flow-deflated.jar --at fixture.FlowFixture
target/release/rxd jar get-class tests/fixtures/jvm/flow-deflated.jar --at 'Lfixture/FlowFixture;' -o target/FlowFixture.json
jq -r '.detail.code' target/FlowFixture.json > target/FlowFixture.java
```

List constant-pool string literals (`--class`/`--filter` narrow the scope):

```sh
target/release/rxd jar strings tests/fixtures/jvm/flow-deflated.jar
target/release/rxd jar strings tests/fixtures/jvm/flow-deflated.jar --class fixture.FlowFixture --filter hello
```

Search cross-class references (string/type/method/field; method/field may use `--class`
owner substring):

```sh
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar -m toJson --kind method
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar -m JsonReader --kind type
target/release/rxd jar xref tests/corpus/jvm/gson-2.11.0.jar -m append --kind method --class java/lang/StringBuilder
target/release/rxd jar xref tests/fixtures/jvm/flow-deflated.jar -m hello --kind string
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
unsupported Java reconstruction returns an explicit error.

## MCP

See [MCP examples](mcp.md) for typed tools, direct path/immutable target calls and full
result consumption. Backend-specific coordinates, offsets and fallback semantics are
preserved by the same query implementation.
