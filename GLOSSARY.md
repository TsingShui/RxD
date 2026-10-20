# Domain glossary

- **rxd**: the Agent-oriented analysis engine and CLI. Format parsing, content
  identities, bounded queries and evidence contracts belong to its shared Rust
  backends and query layer.
- **Arxd**: the Android App host for rxd, now maintained in the standalone
  private repository `TsingShui/Arxd`. It owns the App interface, Agent
  orchestration and Android storage/execution adapters. The old `apps/arxd/`
  copy in this monorepo is archived. Hosting rxd locally does not imply that
  model inference is local.

- **Service definition**: an Arxd workspace-owned immutable v1 launch configuration, identified by canonical-content SHA-256. It survives execution-record cleanup until explicitly removed while unused.
- **Execution instance**: one explicitly admitted run of a service definition, with its own process handle, captured configuration, lifecycle and logs. Restarting creates another instance; no restart happens automatically.
- **Offered MCP catalog**: the tool/schema set captured for model admission, bound to an MCP configuration revision and native connection/catalog identity. Its aliases change when that contract changes; the logical remote-tool identity remains configuration ID plus original name.
