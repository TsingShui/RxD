# ADR 0013: Keep the Arxd Android host in the rxd repository

Status: superseded. Arxd now lives in the standalone private repository
`TsingShui/Arxd`; this record is retained for history.

Status: accepted.

## Context

Arxd is the Android host for rxd. It needs persistent conversations,
process-restart recovery, local rxd execution and an offline-capable Agent
surface without embedding a second analysis implementation.

## Decision

Keep App code and Android host adapters in `apps/arxd/`; keep analysis logic in
the existing Rust backends. Pi is consumed as pinned upstream dependencies.
The host uses Pi-durable's portable core, an Expo SQLite adapter and a local Expo
module that invokes an Android ARM64 rxd executable. It does not embed Node.

Use a deterministic provider to verify the actual harness/storage/tool boundary
without credentials. Label it explicitly. Cross-compilation, host tests, JS
bundling, APK construction and execution on a physical phone remain separate
evidence. A fixed query's captured bytes and engine identity bind safe replay.

## Consequences

App/adapter changes can be reviewed with their shared contracts in one commit.
Arxd has its own dependency lockfile, checks and build process. App-only changes
do not require the entire Rust backend suite; shared-boundary changes still
require relevant integration checks. The existing CLI acceptance gates remain.

Android is the supported host for this app. A shared-library interface, complete
Pi coding-agent extension compatibility, live authentication, additional rxd
backends, iOS and background scheduling require separately scoped decisions.
