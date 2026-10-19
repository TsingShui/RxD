# Handcrafted JVM fixtures

These sources belong to this repository. The class files are compiled analysis
inputs, never executed. Ordinary E2E tests read the checked-in bytes and need no
JDK. Generation used Temurin `javac 21.0.12.1`:

```sh
javac --release 8 -Xlint:-options -g -parameters -d tests/fixtures/jvm tests/fixtures/jvm/FlowFixture.java
javac --release 17 -g -parameters -d tests/fixtures/jvm tests/fixtures/jvm/ModernFixture.java
```

FlowFixture covers arithmetic, wide arguments, casts, reference comparisons,
loops, tableswitch/lookupswitch, arrays (including boolean), invocations,
constructors, field updates, exceptions, synchronized methods and monitor blocks.
ModernFixture contains StringConcatFactory and lambda invokedynamic sites;
these test honest unsupported-Java diagnostics and original bytecode extraction.
InterfaceSuperFixture also tests explicit rejection of qualified interface-super calls.

The add method's independent `javap -c -p` expectation is:
`0: iload_0; 1: iload_1; 2: iadd; 3: ireturn`.

The two flow JARs contain exactly `fixture/FlowFixture.class`, use a fixed ZIP
timestamp `2020-01-01 00:00:00`, and stored/deflate compression respectively.
Regenerate after changing FlowFixture:

```sh
python3 - <<'PY'
from pathlib import Path
import zipfile
root = Path('tests/fixtures/jvm')
for compression, name in [(zipfile.ZIP_STORED, 'flow-stored.jar'), (zipfile.ZIP_DEFLATED, 'flow-deflated.jar')]:
    with zipfile.ZipFile(root / name, 'w', compression=compression) as archive:
        entry = zipfile.ZipInfo('fixture/FlowFixture.class', (2020, 1, 1, 0, 0, 0))
        entry.compress_type = compression
        archive.writestr(entry, (root / 'fixture/FlowFixture.class').read_bytes())
PY
```

Generated Java was additionally assembled into the trusted fixture class and
recompiled with `javac --release 8 -proc:none`, without executing either version.
That check tests source syntax and types, not semantic equivalence.

SHA-256 values for pinned source files, classes and JARs are in [SHA256SUMS](SHA256SUMS).
