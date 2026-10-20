# ADR 0014: One filter, one label

Status: accepted. Applies to every backend and every `-f/--filter` command.

## Context

`-f/--filter` answered a different question per backend, and nothing recorded which.
dex and JVM matched a member's own name; .NET matched the member's name **or** its
declaring type's name. The same query template therefore returned nothing on one
backend and a full page on another:

```sh
rxd jar    methods gson.jar      -f Gson     # 0 rows: no method is named *Gson*
rxd dotnet methods Closure.dll   -f Closure  # 22 rows: none named *Closure*, all owned by one
```

An empty result is the most dangerous answer a search can give: nothing in that output
separates "no such member name" from "this backend also looks at owners". Two backends
disagreeing about one flag means the caller has to know which backend it is talking to
before it can read the answer.

## Decision

1. `-f/--filter` matches **exactly one label**: the record's own name. For a class or
   type listing that is the full name the row prints; for a member listing it is
   `detail.name`; for a string or literal listing it is the value. Never the parent, the
   owner, the declaring type, or any other field of the same row.
2. Narrowing by owner is a **separate, exact** selector that names one identity, not a
   substring: `--class` on every backend whose member listings live under a type. It
   accepts the backend's own spellings of the same identity (`Lcom/foo/Bar;`,
   `com.foo.Bar`, `com/foo/Bar`) and nothing else. Listings always compared it exactly;
   `xref --class` used to fold into its content substring, which was this same defect one
   level down, so it now names one class as well.
3. A question of the form "members whose *owner* matches a pattern" is two calls —
   `classes -f PATTERN` then `methods --class NAME` — not one `-f`. The second call is
   exact, so it cannot silently select a different type.

## Two spellings that stay, because the coordinate is different

- JS `--kind` takes a comma-separated list. Its kinds are the parser's structural kinds
  (`function_declaration`, `object_method`, …), which a caller narrows as a set; the other
  backends' `--kind` names one reference target and stays single-valued. Same flag name,
  same "narrow this listing" job, different arity because the value space differs.
- JS `locate`/`xref` keep `--byte`. A coordinate is what a row printed and can only name a
  range; a byte offset may land *inside* a multi-byte identifier, which is what a cursor
  position is. Both resolve to the same binding, so both stay.

## Consequences

- The same `-f` over the same shape means the same thing on all nine backends, so a
  query template can be reused and an empty result means "no record is named that".
- Case folding stays as decided separately: `-f` folds unless `--case sensitive`, while
  `--class` and `--at` compare identities exactly.
- dotnet renamed its owner flag from `--type` to `--class` for the same reason: one
  question, one spelling. The help text still says "exact full type name", because .NET
  types are not all classes.
- The same reading applied to the identity face beside the filter: the record's own exact
  name is `--name` on every listing that has names (js, pyc, wasm, dex, jar, dotnet),
  replacing `--func-name` and dotnet's method-only `--method`, and giving dex and jar the
  flag their member listings were missing.
- js `find` was the last content search outside the rule: `--string`, case-sensitive, with
  no switch. It is now `-f/--filter`, folded by default, with `--case sensitive` -
  the same face as every other literal search.
- A new backend that lists members gets `--class` and a name-only `-f`; a backend that
  cannot express owner narrowing says so in its design document instead of widening
  `-f`.
