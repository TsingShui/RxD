// Independent Acorn oracle for invocation-anchored registries in the pinned YAML VSIX.
// Parse source only. Ambiguous declarations are excluded rather than guessed.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = dirname(fileURLToPath(import.meta.url));
const { parse, version } = createRequire(import.meta.url)(process.env.RXD_ACORN_MODULE || 'acorn');
if (version !== '8.19.0') throw new Error('Use pinned Acorn 8.19.0');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const provenance = JSON.parse(readFileSync(join(root, 'zip/expectations.json'))).find(x => x.file.endsWith('.vsix'));
const archive = join(root, 'zip', provenance.file);
if (hash(readFileSync(archive)) !== provenance.sha256) throw new Error('Changed VSIX');
const entry = 'extension/dist/languageserver.js';
const bytes = execFileSync('python3', ['-c', 'import sys,zipfile;sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))', archive, entry], { maxBuffer: 8 << 20 });
const source = bytes.toString('utf8');
const ast = parse(source, { ecmaVersion: 'latest', sourceType: 'script' });
const nodes = [];
const functions = new Set(['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression']);
const scope = parents => parents.findLast(n => functions.has(n.type)) || ast;
const declarations = new Map();
function declare(owner, name, value, parents, parameter) {
  if (!declarations.has(owner)) declarations.set(owner, new Map());
  const names = declarations.get(owner);
  if (!names.has(name)) names.set(name, []);
  names.get(name).push({ value, parents, parameter });
}
function walk(node, parents = []) {
  if (!node || typeof node.type !== 'string') return;
  nodes.push({ node, parents });
  if (node.type === 'VariableDeclarator' && node.id.type === 'Identifier') declare(scope(parents), node.id.name, node.init, parents);
  if (functions.has(node.type)) node.params.forEach((p, i) => {
    if (p.type === 'Identifier') declare(node, p.name, node, parents, i);
  });
  for (const child of Object.values(node)) {
    if (Array.isArray(child)) child.forEach(n => walk(n, [...parents, node]));
    else if (child && typeof child === 'object') walk(child, [...parents, node]);
  }
}
walk(ast);
function resolve(name, parents) {
  for (const owner of [...parents.filter(n => functions.has(n.type)).reverse(), ast]) {
    const candidates = declarations.get(owner)?.get(name);
    if (!candidates) continue;
    if (candidates.length !== 1) return null;
    const declaration = candidates[0];
    if (declaration.parameter === undefined) return declaration.value;
    const call = declaration.parents.at(-1);
    return call?.type === 'CallExpression' && call.callee === owner ? call.arguments[declaration.parameter] : null;
  }
}
const byte = index => Buffer.byteLength(source.slice(0, index));
const found = new Map();
for (const { node, parents } of nodes) {
  if (node.type !== 'CallExpression' || node.optional) continue;
  const viaCall = node.callee.type === 'MemberExpression' && !node.callee.computed && node.callee.property.name === 'call';
  const indexed = viaCall ? node.callee.object : node.callee;
  if (indexed.type !== 'MemberExpression' || !indexed.computed || indexed.object.type !== 'Identifier') continue;
  const runtime = scope(parents);
  const runtimeName = runtime.id?.name || (parents.findLast(p => p.type === 'VariableDeclarator' && p.init === runtime))?.id.name;
  const args = node.arguments.slice(viaCall ? 1 : 0);
  if (args.length !== 3 || !runtimeName || args[2].name !== runtimeName || indexed.property.name !== runtime.params[0]?.name) continue;
  if (args[0].type !== 'Identifier' || args[1].type !== 'MemberExpression' || args[1].object.name !== args[0].name || args[1].property.name !== 'exports') continue;
  const table = resolve(indexed.object.name, parents);
  if (!table || found.has(table.start)) continue;
  let entries;
  if (table.type === 'ObjectExpression') entries = table.properties.filter(p => p.type === 'Property' && !p.computed).map(p => [String(p.key.name ?? p.key.value), p.value, p.method ? p : null]);
  else if (table.type === 'ArrayExpression') entries = table.elements.map((v, i) => [String(i), v, null]);
  else continue;
  const modules = entries.filter(([, value]) => value && ['FunctionExpression', 'ArrowFunctionExpression'].includes(value.type)).map(([id, value, method]) => {
    const start = byte((method || value).start), end = byte((method || value).end);
    return { id, kind: method ? 'object_method' : value.type === 'FunctionExpression' ? 'function_expression' : 'arrow_function', start, end, sha256: hash(bytes.subarray(start, end)) };
  });
  if (modules.length) found.set(table.start, modules);
}
const modules = [...found.values()].flat().sort((a, b) => a.start - b.start);
if (modules.length < 2) throw new Error('Expected multiple retained production modules');
const output = {
  generator: { file: 'generate-modules.mjs', sha256: hash(readFileSync(fileURLToPath(import.meta.url))), acorn: version, oracle: 'Acorn lexical-function scopes with unique variable/parameter declarations; literal object/array factories anchored to module/module.exports/self-runtime invocation; UTF-8 ranges and hashes' },
  archive: provenance.file, archive_sha256: provenance.sha256, entry, bytes: bytes.length, sha256: hash(bytes), modules,
};
writeFileSync(join(root, 'modules-expectations.json'), JSON.stringify(output, null, 2) + '\n');
console.log(JSON.stringify({ registries: found.size, modules: modules.length, methods: modules.filter(m => m.kind === 'object_method').length, payload_sha256: output.sha256 }));
