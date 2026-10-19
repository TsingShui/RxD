// Actual Pi SDK session route; invoked only by verify-mcp-clients.py in isolation.
// No prompt(), agent loop, model invocation, trust override or raw MCP transport.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const [packageDir, source, outside, scenario, output] = process.argv.slice(2);
const sdk = await import(pathToFileURL(join(packageDir, 'dist/index.js')).href);
const { createAgentSession, createCodemodeExtension, createMcpExtension,
  createToolSearchExtension, DefaultResourceLoader, ModelRuntime,
  SessionManager, SettingsManager } = sdk;
const cwd = process.cwd();
const agentDir = process.env.PI_CODING_AGENT_DIR;
const settingsManager = SettingsManager.create(cwd, agentDir);
const hooks = [];
const resourceLoader = new DefaultResourceLoader({
  cwd, agentDir, settingsManager,
  noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
  extensionFactories: [
    { name: 'codemode', builtin: true, replaceable: true,
      factory: createCodemodeExtension({ models: false }) },
    { name: 'tool-search', builtin: true, replaceable: true,
      factory: createToolSearchExtension() },
    { name: 'mcp', builtin: true, replaceable: true, factory: createMcpExtension() },
    (pi) => {
      pi.on('tool_call', (event) => { hooks.push({ type: 'tool_call', ...event }); });
      pi.on('tool_result', (event) => {
        hooks.push({ type: 'tool_result', toolName: event.toolName,
          toolCallId: event.toolCallId });
      });
    },
  ],
});
await resourceLoader.reload();
assert.deepEqual(resourceLoader.getExtensions().errors, []);
const modelRuntime = await ModelRuntime.create({
  authPath: join(agentDir, 'auth.json'), modelsPath: join(agentDir, 'models.json'),
  allowModelNetwork: false, refreshOnCreate: false,
});
assert.deepEqual(await modelRuntime.listCredentials(), []);
const { session } = await createAgentSession({
  cwd, agentDir, settingsManager, resourceLoader, modelRuntime,
  sessionManager: SessionManager.inMemory(cwd),
});
const events = [];
const report = { scenario, hooks, events, status: 'failed' };
session.subscribe((event) => {
  if (event.type.startsWith('tool_execution_')) events.push(event);
});
try {
  // Starts native MCP connections. Calling a transport or raw tool definition
  // instead would bypass the session's nested tool pipeline and is not this proof.
  await session.bindExtensions({});
  // There is no model-issued outer tool_call to wait for background discovery.
  // Await native registration before executing the session's wrapped tool.
  if (scenario === 'enabled') {
    const deadline = Date.now() + 15000;
    while (!session.getCallableToolNames().includes('mcp__rxd__js_find') && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    assert.ok(session.getCallableToolNames().includes('mcp__rxd__js_find'), 'native discovery timed out');
  }
  session.setActiveToolsByName(['codemode']);
  const codemode = session.agent.state.tools.find((tool) => tool.name === 'codemode');
  assert.ok(codemode, 'session-wrapped codemode must be active');
  const args = { code: `
    const names = ALL_TOOLS.filter(t => t.name.startsWith('mcp__rxd__')).map(t => t.name);
    ${scenario === 'enabled' ? `
    if (!names.includes('mcp__rxd__js_find')) throw Error('native MCP tool missing');
    const found = await tools.mcp__rxd__js_find({input:{path:${JSON.stringify(source)}},string:'native MCP'});
    const denied = await tools.mcp__rxd__js_find({input:{path:${JSON.stringify(outside)}},string:'native MCP'});
    text({names,found,denied});` : 'text({names});'}
  ` };
  // Explicit authored-history replay, not model-issued use. Pi requires an
  // assistant message for nested calls; satisfy that public history precondition
  // without patching its guard, fabricating a provider stream or starting a turn.
  const authored = { role: 'assistant',
    content: [{ type: 'toolCall', id: 'native-proof', name: 'codemode', arguments: args }],
    api: 'openai-responses', provider: 'authored-fixture', model: 'no-inference',
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
    stopReason: 'toolUse', timestamp: 0 };
  session.sessionManager.appendMessage(authored);
  session.refreshContext();
  report.authoredHistory = authored;
  const result = await codemode.execute('native-proof', args, new AbortController().signal);
  report.result = result;
  const text = result.content.filter((part) => part.type === 'text').map((part) => part.text).join('\n');
  assert.ok(text.startsWith('Script completed'), text);
  // The complete text is retained, not a generated expected snapshot.
  const payload = JSON.parse(text.slice(text.indexOf('{')));
  report.callable = session.getCallableToolNames();
  report.active = session.getActiveToolNames();
  if (scenario === 'enabled') {
    assert.equal(payload.found.isError ?? false, false);
    const found = payload.found.structuredContent;
    assert.equal(found.summary.complete, true);
    assert.equal(found.records.length, 1);
    assert.equal(found.records[0].detail.value, 'native MCP λ');
    assert.equal(payload.denied.isError, true);
    assert.equal(payload.denied.structuredContent.error.code, 'ACCESS_DENIED');
    assert.equal(hooks.filter((e) => e.type === 'tool_call' && e.toolName === 'mcp__rxd__js_find').length, 2);
    assert.equal(hooks.filter((e) => e.type === 'tool_result' && e.toolName === 'mcp__rxd__js_find').length, 2);
    const nested = events.filter((e) => e.type === 'tool_execution_end' && e.toolName === 'mcp__rxd__js_find');
    assert.equal(nested.length, 2);
    assert.ok(nested.every((e) => e.parentToolCallId === 'native-proof'));
  } else {
    assert.deepEqual(payload.names, [], 'persisted config must not be mistaken for session availability');
    assert.ok(!report.callable.some((name) => name.startsWith('mcp__rxd__')));
  }
  assert.deepEqual(session.messages.filter((message) => message.role === 'assistant'), [authored]);
  assert.deepEqual(await modelRuntime.listCredentials(), []);
  report.status = 'passed';
} finally {
  // dispose alone does not emit session_shutdown; close native session resources
  // before invalidating the extension runtime (Pi extension lifecycle contract).
  await session.extensionRunner.emit({ type: 'session_shutdown', reason: 'quit' });
  session.dispose();
  await writeFile(output, JSON.stringify(report, null, 2) + '\n');
}
// Ensure the native configuration remains parseable; no rewriting by this probe.
JSON.parse(await readFile(join(agentDir, 'mcp.json'), 'utf8'));
console.log(JSON.stringify({ scenario, status: report.status }));
