# JSX / TypeScript source corpus

Inputs are parsed, never executed or compiled. Ordinary CLI E2E reads the committed artifacts and expectations offline.

- `babel-parser.d.ts`: original `@babel/parser` 7.29.9 published typings, 9,334 bytes.
- `babel-types.d.ts`: original `@babel/types` 7.29.8 published declarations, 665,892 bytes.
- `login.tsx` and `page.jsx`: repository-authored browser UI scenarios, not production inputs.

`manifest.json` pins artifact SHA-256, npm tarball URLs and integrity, versions, source paths and MIT license paths. Original third-party bytes are unchanged. Declaration packages exercise real published type syntax; they are not evidence about a complete production UI application.

`../generate-typed.mjs` uses pinned Babel parser 7.29.9 independently of Oxc/rxd. It records complete structure hashes, literal positions, selected declaration ranges and local context ranges. The refs oracle selects a plain identifier parameter in a function/signature with no assignment/update/delete and no same-name nested binding; it excludes static keys and classifies type annotations as type uses. It is a restricted lexical sample, not a full TypeScript scope resolver or type checker. Identifier spans exclude trailing annotations. Daily tests do not run the generator or install npm dependencies.

Regenerate from the repository root after an intentional corpus update:

```sh
node tests/corpus/generate-typed.mjs
cargo test --test cli published_types
python3 benches/cli.py --corpus tests/corpus/typed/manifest.json --samples 20
```

The syntactic snapshots are checked in separately from the JS/Acorn corpus so existing JS expectations remain unchanged. Performance reports are kept under ignored `target/`.
