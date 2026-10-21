# rxd-html dependency reference

## html5gum

- Crate: `html5gum`, unmodified upstream; MIT license.
- Pinned version: `=0.8.4` in `Cargo.toml`, locked by the workspace `Cargo.lock`. Default features are disabled; the optional `tree-builder` feature is not used.
- Use: callback tokenizer for bounded, static lexical catalog construction. Input bytes are never executed or fetched from external resources.
- Local correction: upstream 0.8.4 `naive_next_state` in `src/emitters/emitter.rs` maps singular `noframe` to RawText. The private `StateSwitch` adapter in `src/lib.rs` instead maps plural `noframes` to RawText and leaves singular `noframe` in Data state; other declared mappings remain unchanged. The WHATWG in-head rule names `noframes`: https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inhead . This is a local adapter, not a patched upstream crate or an upgrade.

## URL resolution

- Crate: `url`, unmodified upstream; MIT OR Apache-2.0.
- Pinned version: `=2.5.8`, with transitive dependencies locked by the workspace
  `Cargo.lock`.
- Use: local WHATWG URL parsing/joining for `resources`. It has no fetch path.
  Document URLs are explicitly supplied by the caller; local input paths never
  become document bases.
- Script classification follows the type/language steps in
  https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element
  and the JavaScript MIME essence list in
  https://mimesniff.spec.whatwg.org/#javascript-mime-type .
- First-base and invalid/disallowed-base fallback rules:
  https://html.spec.whatwg.org/multipage/semantics.html#the-base-element .
  Selection uses the declared lexical catalog, excluding lexical template
  descendants, not a DOM. CSP, namespace and runtime mutation rules are not applied.

## Lexical scope

`lexical_naive_v1` is not a DOM or browser parser. It assumes scripting-enabled `noscript`, does not implement optional-end closure, template insertion, foreign namespaces, CDATA or integration points. In particular, `tests/fixtures/html/foreign.html` places a script inside SVG without a closing script tag: the naive ScriptData state consumes `</svg><p>later</p>` as text, so `p` is not catalogued. `tests/fixtures/html/template.html` retains lexical parentage instead of building a template content fragment. Catalog completeness describes this bounded lexical profile, not browser completeness.
