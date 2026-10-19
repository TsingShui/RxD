# RadGuard backend: known issues and fix guidance

This file records the remaining quality/correctness issues in the vendored radguard
backend shared by DEX and JVM. None is a wiring problem. The vendored copy under
`third_party/radguard` is authoritative; the upstream repository has a related file but
this one describes the code rxd actually builds.

## Locations and synchronization

| Item | Location |
| --- | --- |
| Vendored, effective code | `third_party/radguard` (see its `FORK.md`) |
| JVM frontend (issue 1) | `crates/rxd-jvm` |
| Upstream | `github.com/MG1937/radguard`, baseline `290de20` |

Workflow: change upstream radguard first, synchronize `src/` and `Cargo.toml` into
`third_party/radguard`, then reapply the changes listed in `FORK.md`. JVM frontend fixes
go directly in `crates/rxd-jvm`.

Keep both gates green:

```sh
# Upstream radguard checkout (DEX output must match Androguard DAD bit-for-bit)
cargo build --release --locked
cargo test --release --all-features --all-targets --locked
python3 -S tests/quality_gate.py --phase 2 --no-build --rust-bin target/release/radguard --diff-workers 4
# phase1 requires mismatch=0; phase2 upper bound 70; currently 0 / 57

# rxd repository root (includes JVM golden baselines)
cargo test --release --offline --test cli
cargo test -p rxd-jvm -p rxd-dex --offline
cargo clippy --offline --workspace --all-targets -- -D warnings
python3 scripts/format-workspace.py --check
```

Two pitfalls:

1. **Line endings**: `third_party/radguard/src/ir/mod.rs`, `src/types.rs` and
   `src/writer.rs` use CRLF; the rest use LF. Script edits must preserve CRLF or produce
   whole-file diffs.
2. The vendored `tests/` is reference-only (`autotests = false`) and is not run; rxd's
   own E2E and golden tests are the gate. After changing vendored `src/`, rerun
   `--test cli` and `-p rxd-jvm`.

## 1. Stack temporaries have family-level type precision

**Status:** structural risk; no invalid output observed in the scanned corpus. Priority:
fix first, because it can surface as intermittent compile failures on real JARs.

`crates/rxd-jvm/src/code.rs` maps JVM stack slots to registers keyed by type **family**
(`stack_family`, `code.rs:1589`), not exact type. The register descriptor is the family
canonical descriptor (`code.rs:823` `stack_slot`):

```rust
Type::Primitive(Boolean|Byte|Short|Char|Int) => "I"
Type::Primitive(Float) => "F" ... Long => "J" ... Double => "D"
Type::Void | Type::Object(_) | Type::Array(_) | Type::Unknown(_) => "Ljava/lang/Object;"
```

Do not revert to exact-type keys: radguard's `merge` unifies the int family with unrelated
references, and `(depth, exact type)` keys put definition and post-join uses in different
registers; that drops casts such as `(byte)` in `narrowBranch`.

Residual issue: reference temporaries are always declared `Object` even when the value
has a concrete type. The gson case

```java
Object v17 = this.getAdapter(typeOfT).read(reader);
```

is harmless because `read()` returns Object. The structural risk is assigning such a
temporary to a concrete field/return position without an implicit narrowing cast:

```java
someStringField = objTypedTemp;
```

The root cause has two layers: the frontend types by family, and radguard SSA
versioning copies the base variable's type (`version_from_metadata` /
`VariableMetadata::versioned` / `variables.version(...)`), so a `Version(u32)` inherits
the base type rather than its own definition type.

Fix options:

- **(a) radguard (recommended):** when creating version variables in
  `dataflow::split_variables_at`/`place_declarations_at`, overwrite `ty` from the
  defining right-hand side (`Value::Binary(b).ty`, `Value::Invoke(i).method.return_type`,
  `Value::NewInstance{ty}`, `Value::CheckCast(c).descriptor`,
  `Value::StaticField/InstanceField(f).ty`, etc.). DEX output changes, so the DAD gate is
  mandatory. If it mismatches, make it a third `SourceStyle` switch for the backend path,
  like `boolean_literals`.
- **(b) rxd-jvm (expensive):** carry variable IDs in dataflow (real phi) and create a new
  variable per definition. Exact typing, but requires reworking `stack_slot` and merge.

Acceptance: `cargo test -p rxd-jvm` (large golden changes, review each) plus `--test cli`;
then scan gson/rhino `get-class` for invalid forms (`Object vN = <arithmetic>`, `Object
vN` assigned to a concrete type).

## 2. Declaration inside `synchronized`/`try`, use outside

**Status:** intentionally not fixed. DAD emits the same shape, and radguard/rxd both state
that output is not guaranteed to compile. Recorded for a future change.

Reproduction:

```sh
./target/release/rxd jar get-class tests/fixtures/jvm/flow-deflated.jar fixture.FlowFixture
#   public static int block(Object lock, int value) {
#       synchronized(lock) {
#           int v17_1 = (value + 2);     // declared inside the region
#       }
#       return v17_1;                    // used outside
#   }
```

`third_party/radguard/src/dataflow.rs:1254` `place_declarations_at` inserts a separate
declaration only when the reaching definition is not inside the common dominator. Here the
definition dominates the use, so `defined_in_common == true` and no declaration is
inserted; the writer inlines it at first use, which structured output places inside the
monitor/try region. CFG dominance does not express lexical nesting.

Rejected attempt: include the use node in the dominance intersection
(`common_dominator(..., statement_location.node)`). It had no effect on this case and
changed three DEX classes, moving DAD phase1 mismatch from 0 to 3.

Correct fix: hoist region declarations in the writer/structuring layer. On entering a
`Try`/monitor region, move variables declared inside but used outside into the entry
node's `declarations`; see `writer.rs` `visit` (`self.write_declaration(variable)`) and
`visit_try_node`/`visit_loop_node` or a new `hoist_region_declarations` pass in
`graph.rs`. This necessarily diverges from DAD; either reset the DAD baseline explicitly
or gate it through `SourceStyle` for the backend path. rxd goldens must be regenerated.

## 3. Supplementary-plane characters escaped as `\u1f600` (invalid)

**Status:** real bug affecting DEX and JVM. Fix opportunistically.

The golden baseline contains:

```sh
grep -n 'u1f600' crates/rxd-jvm/tests/golden/fixture-FlowFixture.class.txt
#   return "\u524d\u7aef\u0000\u1f600";
```

The source is `"prefix\u0000😀"`. Two problems:

1. `\u1f600` is five hex digits and invalid Java (`\uXXXX` is four). The correct form is
   the surrogate pair `\ud83d\ude00`, or a literal emoji.
2. All non-ASCII characters are escaped, unlike dexdec's literal rendering and less
   readable.

Cause: `third_party/radguard/src/writer.rs:1127` `escape_java_string`:

```rust
' '..='~' => escaped.push(character),
character => escaped.push_str(&format!("\\u{:04x}", u32::from(character))),
```

`{:04x}` emits 5–6 digits above U+FFFF. `format!` also allocates for every non-ASCII
character.

Fix: for `> U+FFFF`, emit a UTF-16 surrogate pair (`character.encode_utf16(...)` → two
`\uXXXX`), or a literal under `SourceStyle`. Decide whether non-ASCII stays fully escaped;
full escaping is safest for DAD alignment. If adding readable literals, gate it through
`SourceStyle` so DEX defaults do not change.

Acceptance: `cargo test -p rxd-jvm` (text goldens change; review for escaping only, not
semantics) plus the DAD gate.

## Already completed

- Constructor folding: `crates/rxd-jvm/src/code.rs:447` `fold_constructor_calls` (`new` +
  `<init>` → one Invoke).
- Exceptions use independent registers.
- `boolean` renders by declared type via `SourceStyle::FOREIGN` (DEX keeps `DAD`).
- Single-method failure does not degrade; whole-class failure becomes a stub
  (`crates/rxd-jvm/src/lib.rs:149/162`).
- Declaration scope: issue 2, intentionally unchanged.
