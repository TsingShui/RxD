# Offline hook artifact validation (#135)

This is **test tooling**, not a dependency of the generator. No validator imports,
evaluates or executes its input, invokes Frida, connects to a device, launches a
browser, or installs an extension. `node` executes only this parser/checker and
TypeScript itself. Passing these gates is not evidence of runtime hook behavior.

## Explicit preparation (separate from generation/tests)

```sh
npm ci --prefix devtools/hook-gen --ignore-scripts --no-audit --no-fund
# After this exact lock is cached, preparation can be wholly offline:
npm ci --prefix devtools/hook-gen --ignore-scripts --no-audit --no-fund --offline
```

Setup is the only command here that may download packages. `--ignore-scripts`
prevents package installation scripts from running. Ordinary generation never
calls npm or the validators; tests fail explicitly when tools are absent rather
than fetching them or silently skipping static validation. Installed packages are
ignored; do not commit node_modules, compilers, SDKs, caches or build output.

The existing cached Acorn/TypeScript tooling was preferred during initial setup;
the exact separate preparation used
`npm install --prefix devtools/hook-gen --ignore-scripts --no-audit --no-fund --prefer-offline`.
`package-lock.json` records all resolved tarball URLs and SHA-512 integrity pins.
No Frida runtime package is installed.

| Actual package | Pin | Role / upstream license |
| --- | --- | --- |
| acorn | 8.19.0 | ECMAScript parser, MIT |
| typescript | 5.9.3 | strict noEmit checker and standard libraries, Apache-2.0 |
| @types/frida-gum | 19.12.0 | actual Gum API declarations, MIT |
| frida-java-bridge | 7.0.13 | actual index.d.ts only; runtime module is never imported by validator, LGPL-2.0 WITH WxWindows-exception-3.1 |
| @types/node | 18.19.130 | actual Console/global declarations absent from Gum, MIT |
| @types/chrome | 0.3.5 | actual chrome.runtime.ManifestV3 declarations, MIT |

Transitive versions/integrities are locked too (Chrome filesystem/filewriter/har
and Node undici declarations). npm package notices/licenses remain in each
installed package. No declarations are rewritten or replaced with permissive
stubs. These independent pins are validation identities, not a compatibility
claim inferred from a Frida core version.

Frida checking uses the actual ES2020, Gum, Node and Java bridge declarations.
Node supplies `console`: Gum itself does not declare it; adding lib.dom would
conflict with Gum's File/Worker declarations. Ambient Node declarations do **not**
mean Node APIs exist inside Frida. Providers must only emit actual Gum/bridge APIs.
Web/manifest checking uses actual ES2020/DOM/Chrome declarations, without Gum.
Neither environment uses skipLibCheck, suppressions or substitute any declarations.
Upstream declarations' own types are preserved unchanged.

## Commands

```sh
node devtools/hook-gen/validate.mjs js /path/to/artifact.js
node devtools/hook-gen/validate.mjs js /path/to/first.js /path/to/second.js
node devtools/hook-gen/validate.mjs ts /path/to/artifact.ts
node devtools/hook-gen/validate.mjs ts /path/to/caller-import.ts /path/to/first.ts /path/to/second.ts
node devtools/hook-gen/validate.mjs extension /path/to/published-extension
node --test devtools/hook-gen/validate.test.mjs
```

Multiple source files are concatenated, in order, for one parse/type-check scope;
this checks snippet composition rather than unrelated independent modules.
JS is parsed as ECMAScript 2022 module source (Frida import-bearing ESM accepted).
Extension payloads are parsed as scripts (no runtime module import/export).
TS invokes the pinned `typescript/bin/tsc --project <temporary-config> --pretty false`
with strict=true, noEmit=true, skipLibCheck=false, target ES2020, module ESNext,
Node module resolution and explicit pinned type roots/bridge declaration mapping.
The temporary source/config is removed, never executed. TypeScript process timeout
is 120 seconds; this is a test tooling ceiling, not product latency acceptance.

Provider Rust E2E can `mod support;` and call the shared
`support::{validate_js(&str), validate_ts(&str), validate_extension(&Path)}` in
`crates/rxd-hook-gen/tests/support/mod.rs`. Generate through the public seam first,
then check its actual complete output. To check composition, concatenate actual
generated snippets before passing one string; do not rebuild template fragments
inside test helpers. Check extension publication through public publish_extension,
then pass that resulting directory to the validator.

## MV3 compatibility and validation boundary

Minimum supported Chrome/Chromium is **111**, which supports the statically declared
content-script `world` field. Require `minimum_chrome_version` >=111, Manifest V3,
a single local JS content-script declaration in MAIN world at document_start, and
only top frame (all_frames absent/false). Reference:
<https://developer.chrome.com/docs/extensions/reference/manifest/content-scripts>
(`world`, Chrome 111+); the actual declaration schema is pinned @types/chrome 0.3.5,
not a live schema fetch.

The checker validates actual `chrome.runtime.ManifestV3` via strict TypeScript
`satisfies` **plus** a stricter allowed-key subset: required identity fields,
optional description, explicit minimum version and content_scripts only. This
rejects unrequested permissions, background code, external resources, UI and other
capabilities that Chrome's broader manifest type would allow. Explicit matches
must be nonempty valid match patterns (supported schemes: wildcard/http/https/file/
ftp, or explicit <all_urls>); no implicit all-sites default. Payload references
must be present single-component local .js regular files, no links/subdirectories,
no extra files, no HTML or TS. The generator/Web lane additionally owns matching
its requested options, descriptor/wrapper behavior, wait logic and atomic safe
publication. This checker does not establish page availability, runtime Promise
identity, network behavior of arbitrary code, first-call timing, class loaders,
native ABI truth, device execution or successful browser installation.

## Independent module development

```sh
CARGO_TARGET_DIR=target/hook-gen-foundation cargo test -p rxd-hook-gen --no-default-features --test generation
CARGO_TARGET_DIR=target/hook-gen-dex cargo test -p rxd-hook-gen --no-default-features --features dex --test dex
CARGO_TARGET_DIR=target/hook-gen-native cargo test -p rxd-hook-gen --no-default-features --features native --test native
CARGO_TARGET_DIR=target/hook-gen-web cargo test -p rxd-hook-gen --no-default-features --features web --test web
```

The crate has no root CLI dependency. The native Kuna engine remains separately
optional (`--features native,kuna`) with explicit caller-prepared specs_dir.
`examples/module-driver.rs` reuses the actual thin root Clap argument/output adapter
without compiling the root CLI. It supports Frida DEX signature/index, JS/TS/import/entry,
native function name or explicit 0xHEX static VMA/module selection, and Web snippet/extension
publication with explicit matches/out/wait. `tests/arguments.rs` exercises this same public
argument seam through complete generation/publication or empty-source errors.

```sh
cargo build --release -p rxd-hook-gen --no-default-features --features dex,native,kuna,web --example module-driver
# Existing prepared specs, read explicitly; no global environment mutation or extraction here.
RXD_HOOK_TEST_SPECS=/absolute/path/to/specs target/release/examples/module-driver frida native ./app 0x402000 --lang ts
# Parent target/ must exist; destination must not be nonempty.
target/release/examples/module-driver web --target fetch --emit extension --match 'https://example.com/*' --out target/fetch-extension
```

File inputs are captured once. Raw known-entry native JS and Web never request native resources;
linked TS or required native discovery receive the explicit specs context lazily. Missing features
return explicit provider/engine diagnostics. Source output is complete stdout only; extension
publication leaves stdout empty and reports its destination on stderr. Generation/resolution failure
has no source; a failing stdout write may leave partial bytes, so always check exit status.

Fresh-process release measurements include argument parsing, capture/hash, selection/optional analysis,
complete rendering and stdout file writes or full extension publication. Record every sample, input/output
sizes/hashes, executable identity, environment/specs/cache state, median/P95/>150 ms separately per case.
These are **module-driver**, not root CLI latency or runtime hook evidence. Explicit prepared specs
exclude first extraction; do not imply cold provisioning was measured. No hook/native latency exception is granted.

Input SHA-256 uses the same locked sha2 0.11 as rxd-core, including its default runtime-detected
ARM SHA2 backend and portable fallback (no forced target CPU/features). sha2 0.10 without its
optional asm feature used the software path on aarch64; a pinned 64,193,127-byte Organic Maps APK
exposed that cost. An isolated 10-process-per-language module-driver comparison of the same
CoreComponentFactory constructor retained before medians 169.675 ms JS / 163.822 ms TS (10/10 >150
in each) and after 54.127 / 52.141 ms (1/10 and 0/10 >150). The after first launch was 741.164 ms;
P95 with ten samples is that maximum, not hidden as a warmup. Input and complete output hashes
were unchanged. This is host/input-scoped module evidence, not universal acceleration, cold resource
provisioning or whole CLI acceptance; historical initial/concurrent measurements remain separate.
