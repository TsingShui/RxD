// Independent occurrence oracle: parse pinned JS artifacts; never execute analyzed source.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = dirname(fileURLToPath(import.meta.url));
const { parse, version } = createRequire(import.meta.url)(process.env.RXD_ACORN_MODULE || 'acorn');
if (version !== '8.19.0') throw new Error('Use pinned Acorn 8.19.0');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const manifest = JSON.parse(readFileSync(join(root, 'manifest.json')));
const cases = [];
for (const name of ['jquery-4.0.0-production', 'application-flattened-string-array', 'application-rc4']) {
  const entry = manifest.cases.find(c => c.name === name);
  const bytes = readFileSync(join(root, entry.file));
  if (hash(bytes) !== entry.sha256) throw new Error(`Changed input ${name}`);
  const source = bytes.toString('utf8');
  const ast = parse(source, { ecmaVersion: 'latest', sourceType: entry.source_type || 'script' });
  const tuples = [];
  const byte = index => Buffer.byteLength(source.slice(0, index));
  function walk(node) {
    if (!node || typeof node.type !== 'string') return;
    if (node.type === 'Literal' && typeof node.value === 'string') tuples.push([byte(node.start), byte(node.end), node.value]);
    if (node.type === 'TemplateElement' && node.value.cooked !== null) tuples.push([byte(node.start), byte(node.end), node.value.cooked]);
    for (const value of Object.values(node)) {
      if (Array.isArray(value)) value.forEach(walk);
      else if (value && typeof value === 'object') walk(value);
    }
  }
  walk(ast);
  tuples.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  cases.push({ file: entry.file, bytes: bytes.length, sha256: entry.sha256, occurrences: tuples.length, occurrences_sha256: hash(JSON.stringify(tuples)) });
}
const generator = { file: 'generate-strings.mjs', sha256: hash(readFileSync(fileURLToPath(import.meta.url))), acorn: version, oracle: 'All Acorn string Literal and cooked TemplateElement occurrences, ordered by original UTF-8 start/end; SHA-256 of JSON [start,end,value] tuples, no rxd output' };
writeFileSync(join(root, 'strings-expectations.json'), JSON.stringify({ generator, cases }, null, 2) + '\n');
console.log(JSON.stringify(cases));
