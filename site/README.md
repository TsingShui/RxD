# RxD site

Static, dependency-free pages served by GitHub Pages at
<https://tsingshui.github.io/RxD/>.

## Structure

- `index.html` — RxD overview and backend provenance.
- `examples.html` — ShowCase placeholder; CLI examples live in `docs/demo/`.
- `arxd.html` — notice that Arxd is maintained separately.
- `roadmap.html` — development status and future work.
- `changelog.html` — dated public/site milestones and unreleased work.
- `zh/` — Chinese counterparts of all five pages.
- `assets/style.css` — design tokens, layout, shared subpage styles, responsive rules.
- `assets/main.js` — reveals, sticky nav, mobile menu, language preference, copy buttons. No tracking.
- `assets/lang.js` — early language selection: stored choice, then browser language; honors `?lang=en|zh`.
- `assets/arxd-light.svg`, `arxd-light-1024.png`, `arxd-light-128.png` — the
  light Arxd app mark (cream `#f4f0e8`, ink `#101114`) retained from the former
  App source; `favicon.svg` is the rounded, transparent-corner favicon;
  `favicon.png` is its 64 px fallback. `apple-touch-icon.png` stays square for
  the platform mask.
- `assets/og.png` — 1200×630 social card. Regenerate from the same design
  language when the headline or benchmark claims change.
- `.nojekyll` — upload the directory as-is.

English lives at the root; Chinese lives under `zh/`. The header switch sets
`localStorage.rxd-lang`; `assets/lang.js` redirects on first visit based on the
browser language and honors `?lang=en` / `?lang=zh` for explicit links. Keep
both trees, hreflang pairs and canonical URLs in sync.

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
- Arxd is maintained in the standalone private repository `TsingShui/Arxd`;
  its former App source is no longer included here;
- install commands come from `docs/demo/install.md`, `docs/demo/ci.md`, and the
  root README;
- backend provenance names the actual upstream engine or states that the
  component is RxD-owned; versions and licenses stay in NOTICE and component
  provenance files;
- the public release / curl installer is not yet available; do not present it
  as live until the release is published;
- RoadMap statuses move only after implementation and acceptance; no dates are
  promised;
- ChangeLog records public/main changes; earlier internal acceptance dates stay
  labeled and are never presented as releases;
- English and Chinese pages must carry equivalent claims, commands and links.

## Deployment

`.github/workflows/pages.yml` uploads this directory as the Pages artifact on
pushes to `main` that touch `site/`. Repository settings must use **GitHub
Actions** as the Pages source. The workflow can also be run manually.
