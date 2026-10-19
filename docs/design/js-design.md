# JavaScript backend

Status: implemented.

`rxd js` uses Rust and Oxc for compact bundles and ordinary JavaScript files without
source maps. It provides `outline`, `find`, `identifiers`, `calls`, `members`,
`locate`, `get`, `refs --byte` and `symbols`. The backend maps literals, identifiers,
call sites and static member names to content-bound AST structures, lexical
relationships and original source ranges. It never executes input, performs
deobfuscation or string decryption, restores obfuscator intent, or promises runtime
semantics.

ASAR extraction is handled by `rxd asar`; see [ASAR](asar-design.md) and the
[JavaScript examples](../demo/js.md).

## Scope

The backend handles local valid-UTF-8 JS, JSX, TS, TSX and TypeScript declaration
files, both script and ESM. `outline`/`find` recurse into directories; files are
analyzed independently and cross-file modules/bindings are not resolved. Original
source retains type annotations, JSX, modifiers and declaration syntax; it is never
compiled, transformed or type-checked as a project.

Structural records cover the program root; function declarations/expressions and
arrow functions; class declarations/expressions; methods, constructors, getters and
setters; object-literal methods/getters/setters; and TS type aliases, interfaces,
enums, namespaces, external modules, global declarations and interface method
signatures. A method produces one method record, not a duplicate function record.
Ordinary statement blocks and object literals produce no structural record.
`parent_id` is the nearest containing structure, not a full lexical scope tree. Arrow
functions in class fields are function records whose containing structure is the class.

Deobfuscation and string decryption are permanent non-goals, including control-flow
recovery. Obfuscated input is still searched for readable literals and extracted
byte-exactly; standard JS escape decoding is parsing and is used by `find`. The first
version does not execute input, recover original modules, perform program slicing or
build a complete call graph. Bundler wrapper functions are ordinary functions; module
boundary detection may be added separately later.

## Agent workflow

A typical task locates an API and extracts its implementation:

1. `find` the API string; the result includes the literal position and containing
   structure.
2. `get` the target function's original source.
3. Read the function and identify outer closures or helpers.
4. `get` the parent structure, or `locate` another position by known byte offset.
5. `symbols --within` lists declarations, external bindings and unresolved references
   with exact declaration offsets.
6. `refs --byte` follows an explicit binding and returns declaration/reference
   structures, reducing same-name false matches.

For an existing `rg` hit, `locate` can be used directly. `get` on a function preserves
its access to outer variables; the fragment may not run independently. `parent_id`
provides the closure context.

## Commands

```sh
rxd js outline bundle.js --jsonl
rxd js outline bundle.js --within '<entity-id>' --depth 1 --jsonl
rxd js outline bundle.js --kind object_method --jsonl
rxd js outline bundle.js --name checkFlag --jsonl
rxd js find bundle.js --string '/api/login' --jsonl
rxd js find bundle.js --string 'setInterval' --resolve-static --jsonl
rxd js identifiers bundle.js --name '_0x4e3653' --jsonl
rxd js identifiers bundle.js --name '_0x4e3653' --role reference --within '<entity-id>' --jsonl
rxd js calls bundle.js --name 'AIS3' --jsonl
rxd js members bundle.js --name 'checkFlag' --jsonl
rxd js locate bundle.js --byte 183240 --json
rxd js refs bundle.js --byte 183240 --jsonl
rxd js symbols bundle.js --within '<entity-id>' --jsonl
rxd js get bundle.js --id '<entity-id>'
rxd js get bundle.js --id '<symbol-id>' --json
rxd js get bundle.js --id '<parent-id>' --json
```

| Command | Behavior | Default output |
| --- | --- | --- |
| `outline` | Structures, ranges, names and parent/child relations; filter by kind/name. | JSONL |
| `find` | Case-sensitive literal substring match on decoded string values; returns containing structures. `--resolve-static` also matches bounded AST constant evaluation. | JSONL |
| `identifiers` | Lexical identifier occurrences by exact name (declaration or reference). | JSONL |
| `calls` | Call sites by callee name (direct, member or `new`). | JSONL |
| `members` | Static member names, property keys, JSX attribute names and statically resolvable computed keys. | JSONL |
| `locate` | Innermost structure and ancestors containing a byte position. | JSON |
| `get` | Validate a structure, symbol or fragment ID and extract original source. | Raw bytes |
| `refs --byte` | Single-file lexical binding and same-binding references at a byte position. | JSONL |
| `symbols` | Lexical bindings and unresolved references in a file or selected structure. | JSONL |

`outline` returns the program and all descendants by default. `--within` returns
descendants but not the selected structure itself. `--depth 1` limits to direct
children; no depth means all descendants; `--depth 0` returns none. Record `depth` is
always absolute from program; filter depth is relative to the selected root. Filters
preserve original parent IDs. `--kind` matches exact kinds separated by commas;
`--name` matches exact `name` or context `label` case-sensitively. Both can combine
with `--within`/`--depth`.

`find --string` includes string literals and static template fragments. Ordinary
strings are decoded before matching while original spans are retained. Template
fragments match independently and are marked `dynamic`; concatenation is not inferred
across interpolations or fragments. Empty queries are argument errors; there are no
regex or implicit fuzzy matches. Unpaired UTF-16 surrogates remain valid JSON escapes
and are never replaced, preserving search semantics.

`--resolve-static` additionally evaluates closed AST constant expressions within
fixed step/depth/value budgets. Hits set `detail.literal_kind: static_expression`,
`detail.resolution: static` and `detail.raw` to the full original expression; ordinary
literal hits leave these null. Supported evaluation covers literals, string/template
concatenation, standard conversions such as `String.fromCharCode`/`fromCodePoint`,
a small set of pure string methods (`slice`, `substring`, `substr`, `charAt`,
`toUpperCase`, `toLowerCase`), static array/string indexes and numeric/bitwise
operations, with all arguments statically evaluable. It does not read variable
bindings, execute user functions, parse string arrays, resolve dynamic property names
or rewrite source. Unprovable expressions produce no derived hit. The option is off by
default. A cheap foldability precheck avoids evaluating impossible nodes; on a 3.4 MB
obfuscated input the measured median overhead was about 6 ms, with P95 below 40 ms.

`locate` uses original UTF-8 byte offsets in half-open `[start, end)`; `--byte` must be
less than the file length, and an empty file has no locatable position. The innermost
covering structure wins; ancestors are ordered inner to outer. The program covers
`[0, file_length)`.

`identifiers`, `calls` and `members` are syntax-level location queries by exact name.
They do not copy `find`'s literal semantics or infer bindings. Default is one file;
empty queries are errors; names are case-sensitive full names with no regex/fuzzy
mode.

- `identifiers` emits `record: identifier` with `detail.role` (`declaration` or
  `reference`) and `detail.syntax_kind` (`binding_identifier` or
  `identifier_reference`). `--role` accepts `all|declaration|reference`. Only real
  lexical names match: `foo` in `obj.foo` is not lexical, while `key` in `obj[key]`
  is. `parent_id` is the containing structure; `context_id` is the local fragment.
- `calls` emits `record: call` with `callee`, `call_kind` (`direct`/`member`/`new`),
  `optional` and `argument_count`. It answers only where a name is called, not return
  values, call graphs, cross-file resolution or alias propagation. Computed member
  calls (`obj[key]()`) produce no call hit.
- `members` emits `record: member` with `member_kind` (`static_member`,
  `property_key`, `object_method_key`, `class_field_key`, `class_method_key`,
  `jsx_attribute_name`, `computed_key`) and `key_resolution` (`literal` or `static`
  for computed keys, null otherwise). Computed keys match only string literals or
  statically foldable expressions; variable keys and numeric indexes are handled by
  `identifiers`/normal syntax, not as property names. This is a syntactic property
  position, not runtime object/prototype/type resolution.

All three obey `--max-records` and output-byte budgets, sort by `(start, end)` and keep
content-bound identity. They have no own ID; `parent_id` points at an extractable
structure.

`get` accepts structure IDs, symbol IDs and context fragment IDs and emits the exact
original range without reformatting or extra newlines. `--json` wraps source and
structure metadata in one JSON object. Every command supports `-o FILE` with bytes
identical to stdout. Diagnostics go to stderr; `--debug` reports phase timings. Each
invocation is independent, with no implicit background service or persistent index.

## Recursive scan

`rxd js outline DIR` and `find DIR --string TEXT` recursively scan regular
`.js/.mjs/.cjs/.jsx/.ts/.mts/.cts/.tsx` files (case-sensitive), do not follow symlinks,
and do not apply ignore rules (hidden directories and `node_modules` are included). A
symlinked directory entry is not scanned. Files sort by path; with multiple files each
gets a worker and results are reassembled in path order, so output and budgets match a
serial run byte-for-byte. A single file does not create a thread pool.

`--threads` (default logical CPUs) controls both parallelism and in-flight parsed
inputs, and therefore peak memory: each worker holds at most one input and AST. Serial
and parallel `--perf` counters and phase totals agree (summed in one process; peak RSS
is the maximum). `--depth`/`--kind`/`--name` apply per file; `--within` is single-file
and returns `INVALID_QUERY` (exit 2) for directory input, including empty directories.
`locate/get/refs/symbols/identifiers/calls/members` remain single-file.

Regular-file input keeps its single-file output and does not restrict extension.
`-r/--recursive` is retained for compatibility; directories behave identically with or
without it. An explicit `-r` on a regular file keeps the scan protocol and appends
`scan_summary`; it rejects symlink entries and cannot combine with `--within`.

Recursive JSONL concatenates each file's header/results/summary and appends a final
`scan_summary` with `record, schema_version=2, backend, command, records, complete,
detail`, where `detail` is `files`. Records belong to the preceding header's
`detail.path/source_id`; identical content at different paths is emitted separately.
Success requires exit 0 and a final complete `scan_summary`; empty directories still
return it. Any file failure or resource limit fails the whole invocation atomically,
exposing no partial stdout and preserving the `-o` target.

Input bytes, entities and AST depth are checked per file; record and output-byte
budgets cover the whole invocation, including every header/summary and the final
`scan_summary`. `--max-files` defaults to 10,000; directory depth is capped at 256.
`--perf` accumulates counters/timings and adds a `discover` phase with `source_id:
null`. Each file is read once into a fixed buffer and parser state is released after
processing; no cross-file index is kept.

## Syntax and TS/JSX boundary

`--syntax auto|js|jsx|ts|tsx` and `--source-type auto|script|module` are independent.
Auto syntax uses case-sensitive extensions: `.jsx`→JSX, `.ts/.mts/.cts`→TS,
`.tsx`→TSX, other single files→standard JS; `.d.ts/.d.mts/.d.cts` are declaration
files. An explicit syntax overrides the filename (for extensionless TSX) and never
retries after parse failure. Standard `.js/.mjs/.cjs` remain non-JSX; a `.js` file
containing JSX must use `--syntax jsx`. Directory discovery only covers the eight
extensions and does not follow links; `--syntax` applies to every file.

`header.detail` and JSON `detail` always include `syntax` and `declaration_file`;
extended syntax sets `jsx|ts|tsx` and a boolean, standard JS sets null. `source_type`
is the final script/module mode. Source identity is based on original bytes; entity/
symbol/fragment IDs validate real nodes and are deterministic for the same content,
parse mode, syntax and schema. If copying to another extension changes auto syntax,
the caller must retain the original syntax/mode explicitly.

`find` marks JSX quoted attributes as `literal_kind: jsx_attribute` and JSX child text
as `jsx_text`. Known XML/HTML named and numeric entities are decoded with Oxc's rules;
unknown/invalid entities remain literal. Backslashes inside JSX quotes are not JS
escapes, and React text whitespace is not collapsed; original whitespace/newlines and
spans are retained. JS strings inside JSX expression containers decode normally. JSX
element/fragment/opening/closing/attribute/spread/expression/text nodes can be extracted
via `context_id`; they are not outline function entities.

JSX component names and member-tag object bases use Oxc `IdentifierReference`; ordinary
tags, attribute names and static member names are not lexical references. Paired
opening/closing tags may each contribute a syntactic reference, not two runtime calls.
React component types, prop types and dynamic members are not resolved.

TS string literal types and template type static fragments are `type_string`/
`type_template`; template types do not infer concatenation across interpolations.
Type declaration names/hierarchy support source navigation; enum initializers remain
original expressions without evaluation. Enum members are bindings in their
initializer scope, and identifiers or static string/template member names can be
refs/get source bindings; ordinary JS/JSX static property names remain
`INVALID_TARGET`.

TS/TSX symbol records and get symbol metadata add `space: type|value|type_value`,
meaning the lexical uses Oxc permits, not runtime existence or inferred type. Refs
references and symbols unresolved references add `type_reference`/`value_reference`
booleans; `typeof value` is a type use, which is not a runtime read. Ordinary JS/JSX
always has these fields as null. Missing host/module types may return unresolved rather
than guessing; these are not tsc results.

TS merged/redeclared symbols keep Oxc's canonical first identifier; declaration/value/
type facts are not textually merged into a new type. With multiple declarations,
`symbols.detail`, resolved `refs` header detail and `get` symbol detail expose
`declaration_ids` in original order as structure/fragment IDs. `get(symbol)` stdout
still extracts only the canonical declaration; the Agent can expand other
declarations/overloads. Metadata declaration IDs and top-level records share the
`--max-records` check before collection; summary/perf records only count top-level
results. A forged canonical symbol range from a later declaration is rejected.

`get(symbol)` also covers type aliases, interfaces, enums, namespaces, import-equals
declarations, type parameters and enum members. Type parameters include original
constraint/default source; an enum member can provide its original initializer span.
TS type/namespace structures reset local fragment boundaries; property/call/construct/
index/function/constructor signatures, type parameters, enum members, import-equals
and export assignments can be extracted verbatim. Declaration functions and abstract
methods without bodies still emit their real syntactic range. Type identity,
references, spaces and declaration merging follow pinned single-file Oxc semantics;
cross-module resolution, alias propagation, inference, compilation and runtime
identity are out of scope.

## Identity and names

Each invocation first obtains one fixed source buffer; parsing, searching and
extraction all use it. It never parses one version and reopens another for extraction.

`source_id` is the original file SHA-256. Entity IDs are:

```text
rxd1:js:<source-sha256>:<kind>:<start>:<end>
```

IDs are valid for the same content and schema. They are not promised across builds and
do not rely on potentially duplicate minified names. `get` and `outline --within`
validate the source fingerprint, kind and real structure boundaries; a syntactically
valid range without a matching entity is still an error. Paths are display metadata,
not identity.

Object/class method ranges cover the complete member (modifiers, key, body); function
expressions cover the expression; function declarations cover the declaration node;
program covers the whole input. Exact spans come from the corresponding Oxc nodes and
are fixed by tests. A method and its underlying function never produce two entities on
the same span.

Records distinguish explicit names from context labels:

- `name`: declaration name, named function expression name, or a determinate member
  key; anonymous functions use null.
- `label`: Agent-facing context such as `a` in `const a = () => ...`.
- `label_origin`: `declared`, `binding`, `property`, `assignment` or `anonymous`.

A label is not a unique function name or runtime identity. Dynamic computed keys keep
the key expression range and a null name; expression text is never used to guess the
final property value.

Input is never pre-formatted, CRLF-converted or BOM-stripped, so offsets can extract
directly. If the parser needs adaptation, the adapter preserves or explicitly maps
spans and never silently moves offsets.

## Structured output (schema_version=2)

All structured output follows [ADR 0003](../adr/0003-output-envelope.md) and shared
constructors, with no legacy flat shim. Single-file `outline/find/refs/symbols` JSONL
is header → zero or more records → summary. Success requires exit 0 and
`summary.complete=true`; budget failures return no partial result.

The header contains `record, backend, command, source_id, input_bytes, schema_version,
query, detail`. `input_bytes` replaces `file_bytes`. Query echoes: outline
`within, depth, kind, name`; find `string, resolve_static`; identifiers `name, role,
within`; calls/members `name, within`; refs `byte`; symbols `within`.

Header detail fields:

| Command | Fields |
| --- | --- |
| outline/find | `path, root_id, source_type, analysis, syntax, declaration_file, origin` |
| refs | above plus `resolution, selected, declaration, declaration_ids` |
| symbols | above plus `resolution, within_id` |

`analysis` is `lexical` for refs/symbols and null otherwise. `source_type` is the final
script/module mode. `root_id` always points at program (`parent_id: null`,
`detail.depth: 0`). `origin` is the container adapter's
`container, format, entry, header_id` or null for a direct file; an entry path is the
archive path, while identity and offsets use the entry's original bytes. Refs
`selected` is `name, role, start, end, entity_id, context_id, type_reference,
value_reference`; `declaration` is `start, end, entity_id` or null. These header
objects are not independent records. Summary is `record, records, complete, detail`
with `detail: {}`.

Data records always contain `record, id, source_id, parent_id, context_id, start, end,
detail`; all keys are present and inapplicable values are null. `source_id` is the
owning fixed source SHA-256. Entity and symbol IDs are content-bound; match/reference/
unresolved records have `id: null` and use `parent_id` for the containing structure,
which can be passed to `get`. `symbol.parent_id` is the canonical declaration's
containing structure. Entity `parent_id` keeps the structural parent. Match/reference
`context_id` is the local fragment; entity/symbol/unresolved use null. Start/end are
original half-open UTF-8 byte ranges.

Fixed detail fields:

| Record | Fields |
| --- | --- |
| entity | `kind, name, label, label_origin, depth, flags, key_span` |
| match | `literal_kind, value, dynamic, resolution, raw` |
| identifier | `name, role, syntax_kind` |
| call | `callee, call_kind, optional, argument_count` |
| member | `name, member_kind` |
| reference | `read, write, member_write, type_reference, value_reference` |
| symbol | `name, relation, space, declaration_ids` |
| unresolved_reference | `name, type_reference, value_reference` |
| locate | `complete, entity, ancestors, syntax, declaration_file, origin` |
| source (`get --json`) | `complete, representation, source, entity, symbol, declaration, fragment, syntax, declaration_file, origin` |
| binding (embedded in get) | `name, space, declaration_ids` |
| fragment (embedded in get) | `kind, entity_id` |

`flags` always contains `async, generator, static`. `key_span` is `start, end` or null.
`space`, `declaration_ids` and type/value booleans are null when inapplicable. Match
`resolution`/`raw` are non-null only for `literal_kind: static_expression`. Identifier/
call/member have no own ID; identifier also carries `context_id`, while call/member
`context_id` is null. Names/labels/values preserve JS UTF-16 code units; unpaired
surrogates remain valid JSON escapes and are never lossy-converted.

`locate` returns one `record=locate`; `get --json` returns one `record=source`; neither
has header/summary and both add `backend=js, command, schema_version=2` outside.
`locate` outer id/parent_id/start/end come from the innermost entity and `context_id`
is the local fragment; `detail.entity`/`ancestors` are complete entity bodies ordered
inner to outer. `get` outer id is the requested target, start/end the extracted range,
context_id null; structure targets use the structural parent, symbol targets the
declaration's containing structure, fragment targets the outer fragment.
`detail.representation: original` and `detail.source` are byte-exact. Only the
requested entity/symbol/fragment is non-null. The symbol `binding` record uses the
canonical declaration identifier range; `declaration` is typed (`kind, start, end,
variable_kind, initializer`) and not enveloped; initializer is `start, end` or null.
The fragment parent is the outer fragment, while `fragment.detail.entity_id` is the
containing structure (a different relationship). Default `get` raw bytes stay outside
the envelope and are never modified.

Entities sort by start ascending, end descending, kind; matches by original start;
references by `(start, end)`. Identical input/arguments produce identical output;
timing diagnostics never enter payload.

## Implementation

```text
fixed source buffer
  → UTF-8 and input-mode checks
  → Oxc parse
  → structural traversal and query collection
  → filter, sort and budget checks
  → JSONL or source output
```

Oxc Parser provides a Rust JS/TS parser with spanned AST. Using Oxc does not inherit
its published speed claims; rxd also reads, hashes, traverses and serializes and must
measure itself.

The AST lives in an arena for the request lifetime. One traversal maintains a
containing-structure stack and builds the entity table while collecting string
positions as needed. `get` still parses and validates the real structure, canonical
binding or fragment; it does not slice by ID alone. After validation it extracts from
the fixed buffer without a source printer or print-only AST references.

`--source-type auto|script|module`: auto selects module for `.mjs/.mts`, script for
`.cjs/.cts`, and Oxc unambiguous mode otherwise, returning the final mode. Script
top-level `return` and host extensions are rejected unless a later explicit option is
added.

## Lexical references

`refs FILE --byte N` uses Oxc Semantic binding/reference analysis on demand. It is
single-file only, with no `--id`, `--within` or recursion. `symbols` implements symbol
discovery and content-bound symbol IDs; `refs --id` and cross-file bindings are not
implemented.

`N` must be less than the original length and inside a declaration identifier or
identifier-reference span; it may be inside a multibyte character or escaped
identifier. Whitespace, strings, comments, keywords, static property keys and `foo` in
`obj.foo` return `INVALID_TARGET`; object/destructuring shorthand that is both binding
and reference is selectable. `key` in `obj[key]` is a normal lexical reference.

Refs header detail adds `analysis: lexical`, `resolution`, `selected`, `declaration`:

- `selected` has decoded `name`, `role: declaration|reference`, original `start/end`,
  `entity_id`, `context_id`.
- `resolution: resolved` means a canonical declaration exists in this file;
  `declaration` has its identifier `start/end/entity_id`. Repeated `var` declarations
  still point at the same binding; the canonical position is Oxc's first declaration,
  not every redeclaration. The declaration and initializer are not reference records.
- `resolution: unresolved` means no resolvable lexical declaration; `declaration` is
  null and there are zero references. Unknown globals are not merged by text, and host
  or imported module definitions are not searched; an import alias resolves only to the
  local import binding.
- `resolution: dynamic` means the file contains `with` or a direct `eval(...)` call;
  conservatively the whole file returns dynamic, `declaration: null`, and no
  references. This includes unreachable or shadowed eval calls; it is not a runtime
  impact judgment and eval arguments are never executed/parsed.

Each `record: reference` has `start/end/parent_id/context_id` and detail booleans
`read`, `write`, `member_write`. Simple assignment is write; compound assignment and
increment are read+write; member assignment/update/delete makes the object basis a
read+member_write, not a variable reassignment. References include closure captures but
exclude same-name different-binding positions. No call relation, alias propagation or
dynamic property inference is claimed.

All positions are half-open original identifier ranges, not whole declarations/
statements. `parent_id` (still called `entity_id` inside selected/declaration) is the
innermost extracted structure, not a lexical scope ID; block scopes may share one
structure. Summary records count reference records only; `complete: true` means the
lexical query completed, not runtime reference completeness. Unused resolved
declarations, unresolved and dynamic queries can all succeed with zero records.

`refs`/`symbols` and get symbol paths enable the semantic pass as needed and return its
diagnostics as `INVALID_INPUT`; outline/find/locate and get structure paths stay
parser-only. Structural traversal checks entity and AST-depth budgets first; semantic
nodes above `--max-semantic-nodes` (default 1,000,000) return `RESOURCE_LIMIT` before
index allocation. Reference records are budgeted before collecting positions; output
bytes include header/summary. The node budget is not a parser/peak-RSS limit. Perf
adds a semantic phase and semantic_nodes/symbols/references counts only on those paths.

## Symbol discovery

`symbols FILE [--within ENTITY_ID]` emits single-file JSONL. Default selection is
program; `--within` validates a real structure ID/content identity and selects its
complete original range, including its own declaration identifiers, parameters and all
nested structures. This is a source-region filter, not a "visible symbols in this
lexical scope" query; external bindings not used in the region are not returned, and
class/method keys and parameter defaults are analyzed in their original range. It is
single-file only, with no recursion, depth or name filter.

Only this command builds the same Oxc Semantic index on demand. Header detail adds
`analysis: lexical`, `within_id` (program ID by default) and `resolution:
static|dynamic`. A whole file containing `with` or direct `eval(...)` returns dynamic
and zero records even if the eval is outside the selected region; the rule matches
refs. Static means static lexical analysis, but unresolved names may remain.

Two record kinds:

- `record: symbol`: one per resolved binding; `id/start/end/parent_id` outside,
  detail `name/relation/space/declaration_ids`. `name` is decoded; start/end are the
  canonical declaration identifier's original half-open UTF-8 range; `parent_id` is
  its innermost containing structure. Repeated `var` declarations merge into one
  binding with the refs canonical position. `relation: declared` means at least one
  declaration identifier is in the region; otherwise `external` means a region
  reference to a binding declared outside. External positions still point at the
  original declaration. Same-name different bindings stay separate; imports point only
  at the local import binding.
- `record: unresolved_reference`: one per unresolved identifier reference, with
  `start/end/parent_id` and detail `name/type_reference/value_reference`; no symbol ID,
  no text merging, no host/dynamic guessing. Static property keys, strings, labels,
  `this` and implicit bindings have no declaration record; shorthand/computed real
  bindings/references are handled normally.

Symbol IDs are `rxd1:js:<source-sha256>:symbol:<canonical-start>:<canonical-end>`, not
Oxc internal ordinals. They are stable for the same content, parse options and schema
across calls, paths and within selections, but not across incompatible versions or
different source content. Structure and symbol IDs have different target semantics:
outline/symbols `--within` reject symbol IDs; refs does not accept `--id`; get accepts
symbol IDs and extracts the canonical declaration; get accepts parent_id for the
containing structure. A symbol record's `start` can be passed directly to `refs --byte`.

Both record kinds mix in ascending `(start, end)`. Summary records count symbols plus
unresolved references; complete means the query completed, not resolution/runtime
completeness. Record budgets are checked before collecting identities/positions;
output bytes include header/summary. The semantic node default is 1,000,000
(`--max-semantic-nodes`); entity, depth, input and atomic-output contracts are
unchanged. Perf adds the semantic phase/counts only for refs/symbols/get-symbol.

## Declaration extraction by binding

`get FILE --id SYMBOL_ID [--json]` accepts a canonical symbol ID. On the same fixed
buffer, after entity/depth checks, it builds a node-budgeted semantic index and
validates version, backend, content fingerprint, symbol kind, exact canonical
identifier range and real binding. Reference, later redeclaration, non-canonical
numeric encoding and arbitrary valid slices return `INVALID_ID`; different source
content returns `SOURCE_MISMATCH`. Structure IDs keep the existing parser-only path.

The original extraction range is chosen from the canonical declaration identifier's
AST ancestors:

| Source | `declaration.kind` | Range |
| --- | --- | --- |
| variable declaration | `variable_declarator` | One declarator: full binding pattern and initializer; excludes `const/let/var/using` prefix, sibling declarators and the statement semicolon. |
| ordinary parameter | `parameter` | One complete parameter, including destructuring and top-level default. |
| rest parameter | `rest_parameter` | Complete rest parameter, including ellipsis and binding pattern. |
| catch parameter | `catch_parameter` | Complete catch binding pattern, excluding the catch body. |
| import binding | `import_declaration` | Whole import declaration, retaining local alias and module specifier; the imported module is not read. |
| function/class name | `function_declaration/function_expression/class_declaration/class_expression` | Complete function/class node; does not expand to an outer assignment or export wrapper. |

Default stdout and `-o` are the raw UTF-8 bytes of that range, without prefix,
parentheses or newline and without rewriting into standalone code. Destructuring
bindings may extract the same complete pattern/initializer rather than one inferred
value. Repeated declarations extract only Oxc's canonical first declaration; no later
declaration, assignment or call is appended. `for-in/of` variables return the
declarator, not the iteration input. Output expresses source facts; it never executes,
evaluates or propagates aliases.

Symbol-path `--json` returns `record=source` with `complete/representation: original`
and `symbol`, `declaration`, `source`, not a structural entity:

- symbol: `id/start/end/parent_id` outside, detail `name/space/declaration_ids`;
  start/end are the canonical binding identifier range; parent_id can expand the
  containing structure.
- declaration: `kind/start/end/variable_kind/initializer`; start/end are the actual
  extraction range, separate from symbol identity. `variable_kind` is
  `var/let/const/using/await using` only for variable declarations, otherwise null.
  `initializer` is the full variable initializer or top-level parameter default
  `start/end`, null if absent; nested destructuring defaults stay only in source.
- source is byte-identical to the declaration range. Complete means this declaration
  was extracted, not runtime values, all redeclarations or data-flow completeness. A
  valid real binding can be extracted from a file containing eval/with as syntax only;
  dynamic binding is not recovered. The dynamic symbols query contract is unchanged.

Common input, entity, depth, output and atomic-failure contracts apply. `get
--max-semantic-nodes` defaults to 1,000,000 and only symbol IDs build/limit the
semantic index; structure IDs do not. Perf records is 1; symbol paths include the
semantic phase and whole-file index counts.

## Local syntax fragments

Each find match, locate object and refs selected/reference adds
`context_id: string|null` for local source around the hit. Structural parent IDs/
ranges/order keep their meaning. Refs `declaration` still means the canonical binding
identifier; `context_id` is the selected/reference location and never replaces binding
identity. Symbols output is unchanged.

`context_id` is `rxd1:js:<source-sha256>:fragment.<kind>:<start>:<end>`, a real
content-bound AST fragment identity, not an arbitrary byte slice. The nearest
supported syntactic node is selected:

- variable declaration/declarator; assignment/update expression; ordinary data object
  property/class field.
- parameter/rest/catch parameter; import/export named/default/all declaration.
- expression/return/throw statement; if/switch/switch case/for/for-in/for-of/while/
  do-while/try/with/labeled/break/continue/debugger/empty statement.

Program, functions, arrow functions, classes, class/object methods, getters/setters
are context boundaries; entering one reselects an inner fragment and never reuses an
outer variable initializer or call expression. Ordinary blocks produce no fragment.
Bare directives, function/class names and arrow shorthand without an inner node return
`context_id: null`; the Agent can use parent_id/entity id for the full structure.
Control-statement conditions/discriminants may associate the whole statement. Fragments
are not promised small or complete runtime context.

`get FILE --id CONTEXT_ID [--json]` re-parses the same fixed input, checks entity/AST
depth budgets and validates version, backend, content identity, kind and real AST
range. Missing nodes, wrong kinds, forged ranges and non-canonical numeric encoding
return `INVALID_ID`; changed content returns `SOURCE_MISMATCH`. It uses parser
traversal only, no semantic index; `--max-semantic-nodes` does not apply.
Outline/symbols `--within` still accept only structure IDs. Symbol IDs remain separate.

Default get emits raw fragment bytes without prefix/parentheses/newline. A variable
declarator may omit `const/let/var`; an assignment expression may omit the statement
semicolon; a template hit is associated with the complete enclosing fragment.
Destructuring and dynamic property expressions retain raw source; no value or call
target is inferred.

Fragment-path `--json` returns `record=source` with `complete/representation:
original`, `fragment` and `source`. Fragment `id/start/end/parent_id` outside, detail
`kind/entity_id`: start/end are the extracted range; entity_id is the containing
structure; parent_id is the nearest outer supported fragment within the same context
boundary, or null, and can be expanded by get. For example object property → single
declarator → whole variable declaration; return/throw can expand to an outer if/loop.
Source is byte-exact; complete means extraction succeeded, not business meaning or
runtime completeness.

Find/locate/get fragment paths keep only the current AST-depth context stack and one
requested fragment/match ID; no whole-file fragment table is created. Refs uses the
built semantic node ancestors and the same selection rule; unresolved/dynamic selected
results may still provide a syntactic fragment but do not claim a resolved binding.
Fragments do not add outline entity records or change max-entities. Existing records,
output budgets, directory atomic failure and `-o` contracts remain; serializing
`context_id` is charged to existing output budgets and release benchmarks.

## Performance and validation

Performance work covers process startup, input read/hash, parse/traverse, and
serialization/source output. The native JS path avoids generic compilation,
transformation and project generation. Structural results retain only spans,
relationships and small labels; source is read on demand.

Single-file and recursive scans stay serial in output; file-level parallelism is a
later measured change that must preserve deterministic output. Re-parsing per call is
included in benchmarks; AST-resident query speed is never presented as CLI speed.

Release benchmarks cover about 1 MiB, 10 MiB and 50 MiB authored or authorized real
bundles, recording bytes, structures, build, hardware, tool versions and hashes.
Comparisons include:

| Scenario | Baseline | Metrics |
| --- | --- | --- |
| String entry lookup | `rg -boF` vs `rxd js find` | End-to-end latency and output differences |
| Function structure | Equivalent Oxc/Babel traversal script | Latency, peak RSS, structural accuracy |
| Post-hit extraction | Text search/manual boundaries vs `find → get` | Complete function extraction accuracy and total time |
| Multi-turn Agent analysis | `rg` + file reads, formatted search, `rxd` | Answer/evidence correctness, tool turns, tokens and total time |

Fresh-process and repeated calls are reported separately with cache conditions; OS
file-cache gains are not parser-cache gains. Each sample reports median, tail latency,
peak RSS and output bytes; redirection and real serialization stay in the timing.

Performance is an acceptance requirement: ordinary representative release queries must
complete within 150 ms in most cases. Per-workload samples, median, P95 and >150 ms share
are reported; unrelated workloads are never pooled or padded with tiny fixtures. The
measurement is fresh-process external wall time including startup, worker init,
directory discovery, read/hash, parse, traversal, serialization and output writes; an
AST-resident timing or one `--perf` phase cannot substitute. Hardware, input size,
fresh-process and cache conditions are explicit. Large inputs/directories need
separate scope, measurements and approval; results are never truncated or ID/resource
checks weakened to meet the gate.

If repeated parsing dominates, compare batched queries, an explicit disk index and a
resident session before choosing; startup cost and applicability remain part of the
comparison, and resident results never replace standalone CLI acceptance. Any cache
must validate source content, schema, parser version and parse options. IDs are not
reused across incompatible versions. The first version has no disk cache.

## Errors and resources

Input is untrusted. Reading has an input-byte cap; output has record-count and
serialized-byte budgets; indexing has an entity cap. Limits are checked as early as
possible and continuously during traversal. Defaults come from measured peak memory
and are mandatory for release; unlimited mode is not a substitute.

Syntax errors fail atomically and emit no recovered partial structures. There is no
permissive mode. Parser recursion depth, stack use and cancellation need separate
verification; hostile-depth input is not promised until adequate protection or
isolation exists. `get` is subject to the same parse depth and output checks.

| Code | Condition |
| --- | --- |
| `INVALID_INPUT` | Invalid UTF-8, unsupported input mode, parser error or semantic-query diagnostic |
| `INVALID_OFFSET` | Byte position out of range |
| `INVALID_TARGET` | Refs byte position is not a binding identifier/reference |
| `INVALID_ID` | Invalid ID format or no matching structure |
| `SOURCE_MISMATCH` | ID content fingerprint differs from current input |
| `RESOURCE_LIMIT` | Input/index/output budget exceeded |
| `FORMAT_ERROR` | Result cannot be serialized by contract |
| `IO_ERROR` | Read or write failure |

Exit 0 means complete success including zero results; 1 means input/query/resource/I/O
failure; 2 means CLI argument error. Stderr diagnostics include a stable code and
understandable reason. Ctrl-C returns non-zero and stops output; cancellation is not
success.

JSONL determines matches and checks the total output budget before writing; resource
limits never silently truncate. An I/O error during writing may leave partial bytes;
consumers detect failure by non-zero exit or missing summary. `-o FILE` writes a
same-directory temporary file then replaces the target, preserving the old content on
failure. Raw source stdout has no summary; exit status determines completeness.

Tests use handwritten small JS fixtures covering anonymous/named/arrow/nested
functions, closures, object/class methods, constructors, getters/setters, async and
generators, and confirm methods do not duplicate function records. Boundaries cover
Unicode identifiers, multibyte text, unpaired surrogate escapes, CRLF, BOM, template
interpolation, string escapes, dynamic keys, empty files, invalid offsets and syntax
errors. Every original span must extract the exact code; `get --json` source must be
byte-exact.

Identity tests cover same-name functions, identical content at different paths, changed
content at the same path, forged ranges, wrong kinds and incompatible ID versions.
Filter tests cover `--within`, depth, references when a parent is filtered, and
deterministic ordering. Output tests cover stdout/stderr separation, JSONL header/
summary, empty-result exit, budget failure, write failure, raw bytes and `-o`
consistency. Resource tests include deep nesting and many entities.

Agent acceptance tasks locate an API request function, explain error-trigger
conditions, trace an outer closure variable, and distinguish same-name bindings in
different scopes, checking answers and evidence independently. Shorter output with
lower task correctness is not acceptance.

## MCP projection

`js_outline/find/identifiers/calls/members/locate/refs/symbols/get` reuse existing
implementations. JSONL becomes header/records/summary; recursive directories keep
`files` and `scan_summary` (only for directory-capable `outline`/`find`); get uses the
original source record's `detail.source` with source/CRLF unchanged. Isolated UTF-16
surrogate decoded values are represented losslessly as `{utf16_units: [...]}` in
structuredContent, while ordinary strings stay strings; CLI JSON escaping is unchanged.
A directory target must declare `backend: js`, pin the file set and obey file/discovery/
depth budgets. `identifiers`/`calls`/`members`, like `locate/get/refs/symbols`, are
single-file only.

Shared snapshot, quota, cancellation and complete-response rules are in the
[MCP design](mcp-design.md); the decision is [ADR 0007](../adr/0007-typed-mcp-service.md).
CLI output and exit codes are unchanged by the MCP projection.
