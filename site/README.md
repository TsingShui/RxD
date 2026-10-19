# RxD site

Static, dependency-free pages served by GitHub Pages at
<https://tsingshui.github.io/RxD/>.

## Structure

- `index.html` — RxD overview: hero, supported formats, quick start, backends and MCP.
- `examples.html` — standalone query examples for APK, JavaScript, .NET and more.
- `arxd.html` — standalone Android host page for Arxd.
- `assets/style.css` — design tokens, layout, shared subpage styles, responsive rules.
- `assets/main.js` — reveals, sticky-nav border, copy buttons. No tracking.
- `assets/arxd-light.svg`, `arxd-light-1024.png`, `arxd-light-128.png` — the
  light Arxd app mark (cream `#f4f0e8`, ink `#101114`) from
  `apps/arxd/assets/rxd-mono-black.svg`; `favicon.svg` is the rounded, transparent-corner favicon;
  `favicon.png` is its 64 px fallback. `apple-touch-icon.png` stays square for
  the platform mask.
- `assets/og.png` — 1200×630 social card. Regenerate from the same design
  language when the headline or benchmark claims change.
- `.nojekyll` — upload the directory as-is.

The pages have no build step, webfonts, analytics, or third-party requests.
Open `index.html` directly or serve the directory:

```sh
python3 -m http.server --directory site 8000
```

## Content invariants

Keep these aligned with the repository:

- performance claims come from `benches/README.md` and the root README; do not
  invent benchmark numbers on the site;
- cross-platform claims match the release targets: macOS arm64, Linux
  x86_64/arm64 (glibc ≥ 2.39), Windows x86_64;
- example commands come from `docs/demo/*.md`; `<id>` placeholders are always
  outputs of the preceding query, never invented offsets;
- Arxd claims come from `apps/arxd/README.md` and `docs/design/arxd-design.md`;
  it remains an Android preview without a published APK;
- install commands come from `docs/demo/install.md`, `docs/demo/ci.md`, and the
  root README;
- the public release / curl installer is not yet available; do not present it
  as live until the release is published.

## Deployment

`.github/workflows/pages.yml` uploads this directory as the Pages artifact on
pushes to `main` that touch `site/`. Repository settings must use **GitHub
Actions** as the Pages source. The workflow can also be run manually.
