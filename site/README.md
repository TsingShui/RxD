# RxD site

Static, dependency-free pages served by GitHub Pages at
<https://tsingshui.github.io/RxD/>.

## Structure

- `index.html` — RxD overview, the four "why" points and backend provenance.
- `examples.html` — ShowCase placeholder only. It stays a `<To Be Done>` page;
  CLI usage lives in `src/skills/rxd/SKILL.md` and `rxd --help`, so do not grow
  this page into a command catalogue.
- `arxd.html` — uses the same hero, 420px mark, kicker and section layout as
  `index.html`. It states where the host and engine each live, the Android
  Bionic cross-build artifact and the layer boundaries. Arxd App capabilities
  and releases are not published from this repository, so the page describes
  availability and boundaries, never Arxd feature claims.
- `roadmap.html` — development status and future work.
- `changelog.html` — dated public/site milestones and earlier internal acceptance.
- `assets/style.css` — design tokens, layout, shared subpage styles, responsive rules.
- `assets/main.js` — reveals, sticky nav, mobile menu, copy buttons. No tracking.
- `assets/arxd-light.svg`, `arxd-light-1024.png`, `arxd-light-128.png` — the
  light Arxd app mark (cream `#f4f0e8`, ink `#101114`) retained from the former
  App source; `favicon.svg` is the rounded, transparent-corner favicon;
  `favicon.png` is its 64 px fallback. `apple-touch-icon.png` stays square for
  the platform mask.
- `assets/og.png` — 1200×630 social card. Regenerate from the same design
  language when the headline or benchmark claims change.
- `.nojekyll` — upload the directory as-is.

English is the only site language. There is no `zh/` tree, no language switch
and no `hreflang` pairs; every page is `lang="en"` with one canonical URL.

The pages have no build step, webfonts, analytics, or third-party requests.
Open `index.html` directly or serve the directory:

```sh
python3 -m http.server --directory site 8000
```

## Content invariants

Keep these aligned with the repository:

- performance claims come from `perf-tests/README.md` and the root README; do not
  invent benchmark numbers on the site;
- cross-platform claims match the release targets: Linux x86_64/arm64 (musl),
  macOS arm64, Windows x86_64; Android is a Bionic cross-build, not a desktop
  artifact;
- the backend list and provenance grid match `rxd --help`: `rasc`, `jar`,
  `ilspy`, `wasm`, `pyc`, `unluac-rs`, `dae`, `evmole`, `ark`, `kuna`, `unpack`.
  Command names credit the engine; crates, features and `backend` labels keep
  the format name;
- example commands follow `src/skills/rxd/SKILL.md` and `rxd --help`; `<at>`
  placeholders are always outputs of the preceding query, never invented offsets;
- the output contract is one `# ` column header plus TAB-separated rows, JSON-quoted
  text cells and `\N` for absent values; diagnostics and `--perf` stay on stderr;
- Arxd is maintained in the standalone private repository `TsingShui/Arxd`;
  its former App source is no longer included here;
- install commands come from `docs/install.md` and `docs/ci.md`. No version tag
  has been published, so `releases/latest/download/install.sh` is not live; do
  not present it as working until a Release is published;
- backend provenance names the actual upstream engine or states that the
  component is RxD-owned; versions and licenses stay in NOTICE and component
  provenance files;
- RoadMap statuses move only after implementation and acceptance; no dates are
  promised;
- ChangeLog records public/main changes in descending date order; earlier
  internal acceptance dates stay labeled and are never presented as releases.
  Entries are history: keep them when a backend or feature is later removed,
  and note the removal in the entry instead of deleting it.

## Deployment

`.github/workflows/pages.yml` uploads this directory as the Pages artifact on
pushes to `main` that touch `site/`. Repository settings must use **GitHub
Actions** as the Pages source; without it, `actions/configure-pages` fails with
`Get Pages site failed`. The workflow can also be run manually.
