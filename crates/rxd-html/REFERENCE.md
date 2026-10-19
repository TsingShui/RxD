# rxd-html dependency reference

## html5gum

- Crate: `html5gum`, unmodified upstream; MIT license.
- Pinned version: `=0.8.4` in `Cargo.toml`, locked by the workspace `Cargo.lock`. Default features are disabled; the optional `tree-builder` feature is not used.
- Use: callback tokenizer for bounded, static lexical catalog construction. Input bytes are never executed or fetched from external resources.
- Local correction: upstream 0.8.4 `naive_next_state` in `src/emitters/emitter.rs` maps singular `noframe` to RawText. The private `StateSwitch` adapter in `src/lib.rs` instead maps plural `noframes` to RawText and leaves singular `noframe` in Data state; other declared mappings remain unchanged. The WHATWG in-head rule names `noframes`: https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inhead . This is a local adapter, not a patched upstream crate or an upgrade.

## Lexical scope

`lexical_naive_v1` is not a DOM or browser parser. It assumes scripting-enabled `noscript`, does not implement optional-end closure, template insertion, foreign namespaces, CDATA or integration points. In particular, `tests/fixtures/html/foreign.html` places a script inside SVG without a closing script tag: the naive ScriptData state consumes `</svg><p>later</p>` as text, so `p` is not catalogued. `tests/fixtures/html/template.html` retains lexical parentage instead of building a template content fragment. Catalog completeness describes this bounded lexical profile, not browser completeness.
