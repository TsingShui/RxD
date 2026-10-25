# Upstream maintenance

Project skill: [check-upstream](../../.agents/skills/check-upstream/SKILL.md).
Pi discovers `.agents/skills/` from the project; use `/reload` then
`/skill:check-upstream` in an existing session. No global install is needed.
This is independent of the CLI-use skill embedded by `rxd skill`.

## Read-only discovery

```sh
python3 scripts/check-upstream.py
python3 scripts/check-upstream.py --source kuna --source radguard
python3 scripts/check-kuna-latest.py
python3 -B -m unittest discover -s devtools/upstream -p 'test_*.py'
```

Requires Python 3.9+, Git and network access to the configured remotes. The
stdlib-only checker makes at most one `git ls-remote` request per selected URL,
with up to four concurrent requests and a 30-second timeout per Git command.
`--timeout SECONDS` overrides it. It never fetches objects, initializes/checks out
submodules, updates Cargo, changes pins, builds, or writes review state. Ordinary
builds never invoke it. No scheduler/CI/LLM service is installed by this workflow.
A future caller can run discovery periodically and launch a trusted Agent only
for `changed` rows or outstanding `followups`; it must preserve incomplete status
and not grant code adoption or publishing permissions implicitly.

JSON stdout is the only result protocol. Exit codes:

- **0**: no new review or outstanding recorded follow-up in the selected scope.
- **1**: at least one changed source or recorded follow-up needs action.
- **2**: incomplete/configuration/network/local-pin/provenance failure; other
  source rows may still contain usable results. Never interpret this as current.

Per-source `status` is `current`, `changed`, `reviewed`, `blocked`, or `error`.
`complete` describes discovery coverage, not semantic review completeness.
`action_required` flags known work, not unknown work hidden by failures. A
`reviewed` row can have an older implementation pin, and its `followups` keep the
exit status actionable. `--require-current` also fails for reviewed-but-unadopted
revisions. The Kuna compatibility entry point uses this flag plus
`--require-checkout`, preserving its strict newest-formal-tag gate.

## Registry and scope

`sources.json` is a maintenance registry, **not a build lockfile**. Each row has:

- `id`, relationship, local consumers and a provenance `evidence` path.
- Explicit remote and `track`: `head` (remote HEAD) or `formal-tags` (numeric
  version tags with the given prefix, excluding preview/RC/nightly suffixes).
  These are tag-name policies, not GitHub/NuGet release metadata guarantees.
- `pin.revision`: full upstream baseline SHA, checked against provenance text;
  `pin.gitlink`: actual stage-0 index gitlink, checked against `.gitmodules`
  for URL identity and an initialized checkout for tracked modifications/mismatch;
  or `pin.tag` for version-only
  provenance, resolved from upstream. Both lightweight and annotated tags work.
- A `blocked` reason where the upstream identity/baseline is unknown.

Gitlink discovery works without initialized submodules unless
`--require-checkout` is selected; `pin.checkout` records that distinction. It
uses the index (including staged upgrades), not the previous committed gitlink.
No local tag fetch is required. `latest` identifies the selected remote ref and
peeled commit. HEAD changes are reported as **different**, not proven newer;
ancestry and file correspondence require the Agent's subsequent investigation.
`compare_url` is only a GitHub navigation link and uses three-dot semantics.
A tag-only baseline resolves remotely and cannot detect historical retagging;
verify release source before adaptation. Literal checks catch simple stale
registry entries but do not prove a fork/port implements that entire baseline.

Initial coverage: Kuna; RadGuard and its DAD origin; axmldecoder; the imported
Rust pycdc snapshot; JADX-derived JVM code; dotscope and its production validator;
the product NativeAOT ILSpy baseline.
The pycdc remote/full revision and the original DAD baseline are unknown and
intentionally make an all-source run incomplete. Resolving provenance is work,
not permission to invent a relationship or hide the source.

This is **not** a general Cargo security audit or a recursive scan of every
transitive crate, fixture, spec or reference submodule. Unmodified crates.io
parser dependencies (Oxc, wasmparser, EVMole, etc.) remain pinned by Cargo and
their existing reference policies; add explicit registry entries if maintenance
tracking is requested. Changing tracked scope/policy requires provenance and
review, not silent automatic discovery by matching repository names.

## Durable review cursor (optional)

`reviews.json` starts empty: discovery is not a review. When a complete review
is explicitly recorded, add an entry such as the following (placeholders must
be replaced, and the report must contain both full object IDs):

```json
{
  "schema_version": 1,
  "reviews": {
    "radguard": {
      "baseline_revision": "FULL_IMPLEMENTATION_BASELINE_SHA",
      "revision": "FULL_REVIEWED_UPSTREAM_SHA",
      "report": "devtools/upstream/reviews/YYYY-MM-DD-radguard.md",
      "followups": ["Unimplemented actionable/deferred change and its tracking reference"]
    }
  }
}
```

The report accounts for the complete range, ancestry, code/tests inspected,
local mappings, licenses, decisions (`adopt`, `adapt`, `already-covered`,
`not-applicable`, `defer`), validation and any remaining actions. Do not advance
this cursor past unreviewed gaps. A matching cursor avoids repeatedly reviewing
an unchanged remote, but never hides recorded follow-ups, including after HEAD
moves again. Missing/invalid evidence is an error, not a successful skip.

The implementation baseline stays in its original FORK/NOTICE/source manifest
or gitlink and is never advanced for ignored/reviewed changes. Record selective
adoptions individually unless the entire upstream baseline was integrated.
Temporary discoveries, downloaded history and validation outputs belong under
ignored `target/upstream/`; only intentionally recorded review evidence belongs
in `devtools/upstream/reviews/`.
