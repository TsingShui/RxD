# Pinned CPython static-query corpus

The 3.12.14 `graph` and `extended` are locally authored synthetic diagnosis
sources, compiled by
trusted final CPython using `compile(..., dont_inherit=True, optimize=0)` and
serialized by `marshal.dumps(..., 4)`. Neither input is imported, called, or executed.
`CodeType.replace` appends shared immutable containers, strings, and code aliases
without modifying the compiler-produced instruction indexes. Synthetic sources
and the locally authored generator use SYNTHETIC-LICENSE (MIT); CPython format
reference license is retained as CPYTHON-LICENSE from official v3.12.14.

`3.12.14/manifest.json` pins exact compiler binary/version/magic, source, PYC,
raw marshal, oracle, and generator SHA-256. It is a filtered copy of the original
preparation manifest, whose SHA-256 is also retained. Only graph and extended
artifacts needed for static navigation/FETCH are imported.
The unchanged generator also has representative generation options; those outputs
are not implied to be present here. PYC headers cover deterministic timestamp
and PEP552 checked
and unchecked hash modes. Ordinary E2E checks consume only saved files, offline.

`graph.oracle.json` obtains code metadata, unique preorder nodes and all graph
edges directly from CPython code objects. Instruction, line, exception, and
localsplus expectations come from that release's `dis`/CodeType data. Original
marshal/code/payload spans come from a small independent scanner based on pinned
CPython marshal.c grammar, never from rxd or pycdc output. It records the actual
serialized reference sites separately. Source basis:
https://github.com/python/cpython/blob/v3.12.14/Python/marshal.c
https://github.com/python/cpython/blob/v3.12.14/Lib/dis.py
https://github.com/python/cpython/blob/v3.12.14/Lib/opcode.py

Reproduce with the pinned trusted interpreter recorded in the manifest:

```sh
/path/to/cpython-3.12.14/bin/python3.12 -I tests/corpus/pyc/generate.py --output target/pyc-corpus
```

The generator writes `target/pyc-corpus/3.12.14/`; destination-specific metadata
can differ, but graph/extended source/marshal/PYC/oracle artifact bytes are deterministic.
Do not import or execute generated inputs. `--representative` reads/compiles exact
standard-library source only, checks analyzed modules remain absent from
sys.modules, and records official exact-tag URLs/hashes and the PSF license.
Actual stdlib modules are used separately for release latency; this synthetic
graph is not production performance acceptance.

`extended` uses300 distinct constant strings/local slots to obtain real compiler-produced
EXTENDED_ARG prefixes and expanded operands; it remains a synthetic diagnostic
fixture, not a production latency workload.

`refs` is a separate synthetic compile-only diagnosis source covering all24 final3.12
explicit name opcode families: global/name/local/free/attribute reads, writes and
deletes, import name/from, cell creation/capture, read-and-clear and annotation-scope
dict-or-global/deref reads. `generate_refs.py` uses the existing independent trusted
helper, and `3.12.14/refs.manifest.json` pins both generator hashes, exact compiler,
source/PYC/marshal/oracle sizes and SHA-256. Its source/generator use the same MIT
SYNTHETIC-LICENSE and retain CPython attribution. Reproduce offline preparation:

```sh
/path/to/cpython-3.12.14/bin/python3.12 -I tests/corpus/pyc/generate_refs.py --output target/pyc-refs-corpus
```

Never import or execute the analyzed source/code. Ordinary tests read the saved
oracle and mutate fixed input bytes only; no installed compiler is required.

## Final-format matrix (ticket #38)

`matrix/<exact-version>/` pins final CPython 3.8.20, 3.9.6, 3.10.16,
3.11.11, 3.12.14 and 3.13.2. Each unchanged original preparation manifest records
exact compiler binary SHA-256, final release info, magic and every source/marshal/
PYC/oracle hash. The copied manifest adds `original_manifest_sha256` and optional
storage metadata; original artifact hash/size fields are never rewritten.
`matrix/interpreters.json` retains independent opcode category/opmap metadata and
trusted format-tool hashes. `matrix/source-verification.json` records byte-exact
verification of all30 stdlib sources against official exact CPython tags.

Each version contains graph/extended synthetic diagnosis plus actual released
`ast`, `configparser`, `email.policy`, `email.parser` and `dataclasses` source and
compile-only PYC/raw marshal. Every version has its exact official CPython PSF
license as `CPYTHON-LICENSE`; stdlib sources and compiled derivatives retain that
license. Synthetic graph/extended/semantics and authored preparation helpers use
the existing MIT `SYNTHETIC-LICENSE`. No provenance or relationship with the
imported Rust engine is inferred from these independent fixtures.

Large oracle JSON is stored losslessly as `.oracle.json.gz` (gzip level9, mtime0).
Its original uncompressed hash/size remains in the manifest, with separate stored
file/hash metadata. Ordinary CLI E2E decompresses the saved data offline, without
requiring any of the six interpreters. Reproduction may change packaging bytes
across compressor versions; compare the original decoded oracle hash.

Reproduce each exact version using its manifest's compiler, without importing or
executing any analyzed code:

```sh
/path/to/pinned-final-cpython -I tests/corpus/pyc/generate.py --representative --output target/pyc-matrix
/path/to/pinned-final-cpython -I tests/corpus/pyc/generate_matrix.py --output target/pyc-matrix
```

`generate_matrix.py` adds cross-version `semantics` diagnosis for class closure
reads, a context manager with genuine WITH_EXCEPT_START, all3 final3.13 packed
FAST families, and cell/free comprehension slots. `semantics.manifest.json`
pins both independent helper and generator hashes and each artifact before
compression. Source is only passed to compile/marshal/dis; no function, class or
module is instantiated/imported/executed.

The value JSON encoder sorts sets/frozensets for reproducibility. Those sorted
values are a literal multiset oracle, **not** a marshal child-order/path oracle.
Tests compare multisets and original byte spans, plus independently worked
marshal bytes that encode `frozenset["z","a"]` in that exact order for path IDs.
Tuple order and original reference/code ranges are independently recorded;
raw prefix/cache bytes and operand/line/exception expectations come from CPython
format tools, never from rxd or the imported decompiler.

`benches/pyc_versions.py` audits original artifact hashes, verifies every original
body in all30 actual release modules against saved independent code/operand/
location/exception expectations, checks name/literal navigation and measures
outline/strings/refs/small returned-ID FETCH as separate workloads. Each command
uses at least20 new CLI/worker processes; decompile exit/stdout is compared with
an immutable prior binary and incomplete/failed outcomes stay explicit. Every
sample and any failure/>150ms workload remain under ignored target; no source
semantic equivalence, automatic performance exception or final #39 closure is
claimed by static-format verification.
