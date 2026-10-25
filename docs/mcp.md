# Local typed MCP

Default source builds include MCP (Kuna remains optional); a reduced build needs
`--features mcp` and the desired backends. `rxd mcp` is the stdio server; it
starts no client, writes no client configuration and initializes no engine:

```sh
rxd mcp
# Optional restrictions; repeat for additional files/directories:
rxd mcp --allow-path /workspace --allow-path /inputs/app.apk
```

All local input paths must be absolute, independent of the server's working
directory. Without `--allow-path`, all local paths are permitted. With grants,
only the named files and descendants of the named directories are permitted;
relative inputs fail with `INVALID_QUERY` and out-of-scope inputs with `ACCESS_DENIED`.
Register the exact absolute executable and argv array in the client's user configuration
(Pi/CC JSON `mcpServers.rxd`; Codex TOML `mcp_servers.rxd`):

```json
{"mcpServers":{"rxd":{"command":"/absolute/path/rxd","args":["mcp"]}}}
```

Never put a shell command string in an argv field. See
[onboarding](install.md#agent-onboarding) for path grants, client reload and
native verification. Persistence alone is not a connected session; Pi shell
listing does not prove its session route when extensions replace or disable
native MCP.

The server uses stdin/stdout for JSON-RPC; diagnostics go to stderr. Legacy clients initialize
normally. A current discovery request can carry this metadata in `params._meta`:

```json
{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"analysis-agent","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}
```

Command `--help` includes minimal examples, parameter sources and result routing.
MCP descriptions inherit the same guidance; `tools/list` publishes required fields,
exclusive selectors and enums. JSON output is automatic: omit `json`, `output` and
`perf`. Invalid argument combinations fail with `INVALID_QUERY` before input capture.
Ordinary tools build argv from the clap definition and execute the same CLI handlers.
Results go to an explicit bounded buffer, not thread-local stdout capture; JSON
projection and Native/extraction adapters retain their existing behavior.

Use the exact schemas returned by `tools/list`. Tool names follow CLI namespaces (JVM uses
`jar_*`); all tools supported by the build are exposed automatically. For codemode, this glue assumes
a client `callTool` method returning the MCP result object:

```js
async function call(name, args) {
  const result = await client.callTool({name, arguments: args});
  if (result.isError) throw result.structuredContent.error;
  return result.structuredContent;
}
const opened = await call("rxd_open", {path: "/workspace/app.js", backend: "js"});
try {
  const input = {target_id: opened.target_id};
  const hits = await call("js_find", {input, filter: "/api/login"});
  // A stream is `{records: [...]}`; `at` identifies a row and `context_at` names
  // the structure that reported it.
  const sources = await Promise.all(hits.records.map(hit =>
    call("js_slice", {input, at: hit.context_at})));
  const originalText = sources.map(source => source.detail.source);
} finally {
  await call("rxd_close", {target_id: opened.target_id});
}
```

The copied input stays fixed even if the original path changes. Handles belong to one
server process and stay valid until `rxd_close` or shutdown; there is no TTL. Direct
one-shot calls need no open:

```js
const out = await call("jar_methods", {
  input: {path: "/workspace/gson.jar"},
  class: "Lcom/google/gson/JsonPrimitive;", filter: "isNumber"
});
const method = out.records.find(row => row.record === "method");
const body = await call("jar_get_method", {
  input: {path: "/workspace/gson.jar"}, at: method.at, raw: true
});
```

For repeated calls use an open JAR target to avoid repeating input capture.
Returned method coordinates remain file-scoped; use them only with that target
or unchanged input bytes.

ASAR child targets compose without caller-managed paths:

```js
const parent = await call("rxd_open", {path: "/workspace/app.asar", backend: "asar"});
const extracted = await call("asar_extract", {
  input: {target_id: parent.target_id}, at: "main.js", include_bytes: true
});
const child = extracted.child_target;
await call("rxd_close", {target_id: parent.target_id});
try {
  const structure = await call("js_entities", {input: {target_id: child.target_id}});
  // extracted.bytes_base64 is the exact entry; child.source_id is its identity digest.
} finally {
  await call("rxd_close", {target_id: child.target_id});
}
```

ZIP uses the same child-target contract. For example, extract a JAR from an AAR
and analyze its immutable bytes without a caller-managed output file:

```js
const parent = await call("rxd_open", {path: "/workspace/sdk.aar", backend: "zip"});
let child;
try {
  const files = await call("zip_entries", {input: {target_id: parent.target_id}, filter: "classes.jar"});
  const entry = files.records.find(row => row.detail.path === "classes.jar");
  const extracted = await call("zip_extract", {input: {target_id: parent.target_id}, at: entry.at});
  child = extracted.child_target;
  const classes = await call("jar_classes", {input: {target_id: child.target_id}});
} finally {
  if (child) await call("rxd_close", {target_id: child.target_id});
  await call("rxd_close", {target_id: parent.target_id});
}
```

CLI `--entry` is not an MCP argument; use extraction and the returned child target.

Directory targets require `backend: "js"`. A complete ambiguous candidate result is a
domain error, not one arbitrarily chosen definition. EVM always requires explicit
`encoding: "raw"` or `"hex"`. Kuna functions/decompile are available only in linked builds.
No result creates new DEX/EVM entity IDs or guarantees decompiled semantic equivalence.

Backend analysis uses the same finite, input-scaled budget policy as the CLI. There
are no MCP-only work, instruction, record, file, entry, capture or target-count quotas
and no per-query `limits` object. Ordinary analysis has no input-derived work,
instruction, individual expanded-entry or serialized-output quotas;
MCP still enforces shared in-flight storage. Structural depth, expansion and address checks remain.

`rxd mcp --help` lists the adjustable service protections: incoming frames,
active requests, workers/threads, deadlines, shared snapshot storage (including
metadata and empty files), shared in-flight request/IPC/response storage, and sampled
Kuna worker RSS. Snapshot and in-flight envelopes default to 1 GiB each; these are
accounted storage envelopes, not hard process RSS limits. Failed or cancelled
requests release their reservations and undelivered child targets. Raise service
settings explicitly for larger workloads. ILSpy retains its native-layer limits.

A standard-library relocated smoke is available during development:

```sh
python3 scripts/mcp-smoke.py --binary target/release/rxd
# Complete linked distribution:
python3 scripts/mcp-smoke.py --binary target/release/rxd --expect-native
```

Correctness/CI configuration is not a latency result;
measure frozen release binaries with [MCP runner](../benches/README.md#mcp).
