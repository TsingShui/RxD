# Local typed MCP

Default source builds include MCP (Kuna remains optional); a reduced build needs
`--features mcp` and the desired backends. Register this existing executable in
Pi, CC or Codex user configuration, without starting a client or an engine:

```sh
rxd mcp install -a pi -a cc -a codex --base /workspace --root /workspace --preview
rxd mcp install -a pi -a cc -a codex --base /workspace --root /workspace
# Direct stdio launch remains available:
rxd mcp serve --root /workspace --base /workspace --backends js,jar,wasm
```

Replace `/workspace` with an existing authorized input directory. See
[onboarding](install.md#agent-onboarding) for selection/detection, overrides,
preview/no-op/replace, client reload and actual native no-model verification.
Persistence alone is not a connected session; Pi shell listing does not prove its
session route when extensions replace or disable native MCP.

Manual stdio fallback for unsupported clients (adapt to their native format;
replace the executable and authorized base/root):

```json
{"mcpServers":{"rxd":{"command":"/absolute/path/rxd","args":["mcp","serve","--base","/workspace","--root","/workspace"]}}}
```

The server uses stdin/stdout for JSON-RPC; diagnostics go to stderr. Legacy clients initialize
normally. A current discovery request can carry this metadata in `params._meta`:

```json
{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"analysis-agent","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}
```

Use the exact schemas returned by `tools/list`. Tool names follow CLI namespaces (JVM uses
`jar_*`); build/startup filters control availability. For codemode, the following glue assumes
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
  const hits = await call("js_find", {input, string: "/api/login"});
  if (!hits.summary.complete) throw new Error("incomplete query");
  const sources = await Promise.all(hits.records.map(hit =>
    call("js_get", {input, id: hit.context_id})));
  const originalText = sources.map(source => source.detail.source);
} finally {
  await call("rxd_close", {target_id: opened.target_id});
}
```

The copied input stays fixed even if the original path changes. Handles expire and belong
to one server process. Direct one-shot calls need no open:

```js
const out = await call("jar_outline", {
  input: {path: "/workspace/gson.jar"},
  class: "com/google/gson/JsonPrimitive", method: "isNumber"
});
const method = out.records.find(row => row.record === "method");
const body = await call("jar_get_method", {
  input: {path: "/workspace/gson.jar"}, id: method.id, bytecode: true
});
```

For repeated calls use an open JAR target to avoid repeating input capture. Each returned
method ID still binds to its actual class bytes, rather than the container handle.

ASAR child targets compose without caller-managed paths:

```js
const parent = await call("rxd_open", {path: "/workspace/app.asar", backend: "asar"});
const extracted = await call("asar_extract", {
  input: {target_id: parent.target_id}, file: "main.js", include_bytes: true
});
const child = extracted.child_target;
await call("rxd_close", {target_id: parent.target_id});
try {
  const structure = await call("js_outline", {input: {target_id: child.target_id}});
  // extracted.bytes_base64 is the exact entry; child.source_id is its SHA-256.
} finally {
  await call("rxd_close", {target_id: child.target_id});
}
```

Directory targets require `backend: "js"`. A complete ambiguous candidate result is a
domain error, not one arbitrarily chosen definition. EVM always requires explicit
`encoding: "raw"` or `"hex"`. Kuna functions/decompile are available only in linked builds.
No result creates new DEX/EVM entity IDs or guarantees decompiled semantic equivalence.

A standard-library relocated smoke is available during development:

```sh
python3 scripts/mcp-smoke.py --binary target/release/rxd
# Complete linked distribution:
python3 scripts/mcp-smoke.py --binary target/release/rxd --expect-native
```

Full finite defaults, UTF-16 projection, errors and future HTTPS boundary are documented in
[MCP design](../design/mcp-design.md). Correctness/CI configuration is not a latency result;
measure frozen release binaries with [MCP runner](../../benches/README.md#mcp-release-measurements).
