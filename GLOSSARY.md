# Domain glossary

- **rxd**: the Agent-oriented analysis engine and CLI. Format parsing, content
  identities, bounded queries and evidence contracts belong to its shared Rust
  backends and query layer.
- **Arxd**: the Android App host for rxd, now maintained in the standalone
  private repository `TsingShui/Arxd`. It owns the App interface, Agent
  orchestration and Android storage/execution adapters. The old `apps/arxd/`
  copy in this monorepo is archived. Hosting rxd locally does not imply that
  model inference is local.
