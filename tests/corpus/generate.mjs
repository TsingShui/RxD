// Build immutable E2E fixtures and independent Acorn expectations. Never execute fixtures.
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'acorn';
import { minify } from 'terser';
import Obfuscator from 'javascript-obfuscator';

const root = dirname(fileURLToPath(import.meta.url));
const hash = data => createHash('sha256').update(data).digest('hex');
const packages = Object.fromEntries(['acorn', 'javascript-obfuscator', 'terser', 'jquery', 'lodash', 'vue']
  .map(name => [name, JSON.parse(readFileSync(join(root, 'node_modules', name, 'package.json'))).version]));
mkdirSync(join(root, 'artifacts'), { recursive: true });
mkdirSync(join(root, 'licenses'), { recursive: true });

function oracle(source, needle) {
  const ast = parse(source, { ecmaVersion: 'latest', sourceType: 'script' });
  const offsets = new Map();
  const byte = offset => {
    if (!offsets.has(offset)) offsets.set(offset, Buffer.byteLength(source.slice(0, offset)));
    return offsets.get(offset);
  };
  const stack = [];
  const entities = [];
  const matches = [];
  const ref = entity => entity && [entity.kind, entity.start, entity.end];
  const name = (node, computed = false) => computed ? null
    : node.type === 'PrivateIdentifier' ? `#${node.name}`
    : node.type === 'Identifier' ? node.name
    : node.type === 'Literal' ? String(node.value) : null;

  function visit(node, parent, property, templateDynamic = false) {
    if (!node || typeof node.type !== 'string') return;
    let kind = null;
    let entityName = null;
    let flags = { async: false, generator: false, static: false };
    const methodFunction = (parent?.type === 'MethodDefinition' && property === 'value')
      || (parent?.type === 'Property' && property === 'value'
          && (parent.method || parent.kind === 'get' || parent.kind === 'set'));
    if (node.type === 'Program') kind = 'program';
    else if (node.type === 'FunctionDeclaration' || (node.type === 'FunctionExpression' && !methodFunction)) {
      kind = node.type === 'FunctionDeclaration' ? 'function_declaration' : 'function_expression';
      entityName = node.id?.name ?? null;
      flags.async = node.async; flags.generator = node.generator;
    } else if (node.type === 'ArrowFunctionExpression') {
      kind = 'arrow_function'; flags.async = node.async;
    } else if (node.type === 'ClassDeclaration' || node.type === 'ClassExpression') {
      kind = node.type === 'ClassDeclaration' ? 'class_declaration' : 'class_expression';
      entityName = node.id?.name ?? null;
    } else if (node.type === 'MethodDefinition') {
      kind = { constructor: 'constructor', method: 'class_method', get: 'getter', set: 'setter' }[node.kind];
      entityName = name(node.key, node.computed);
      flags = { async: node.value.async, generator: node.value.generator, static: node.static };
    } else if (node.type === 'Property' && (node.method || node.kind === 'get' || node.kind === 'set')) {
      kind = { init: 'object_method', get: 'getter', set: 'setter' }[node.kind];
      entityName = name(node.key, node.computed);
      flags.async = node.value.async; flags.generator = node.value.generator;
    }
    if (kind) {
      const entity = { kind, start: kind === 'program' ? 0 : byte(node.start),
        end: kind === 'program' ? Buffer.byteLength(source) : byte(node.end),
        parent: ref(stack.at(-1)) ?? null, depth: stack.length, name: entityName, flags };
      entities.push(entity);
      stack.push(entity);
    }
    const value = node.type === 'Literal' && typeof node.value === 'string' ? node.value
      : node.type === 'TemplateElement' ? node.value.cooked : null;
    if (value !== null && value.includes(needle)) {
      const key = ['Property', 'MethodDefinition', 'PropertyDefinition'].includes(parent?.type)
        && property === 'key';
      matches.push({ literal_kind: node.type === 'TemplateElement' ? 'template' : key ? 'property_key' : 'string',
        value, start: byte(node.start), end: byte(node.end), dynamic: node.type === 'TemplateElement' && templateDynamic,
        entity: ref(stack.at(-1)) });
    }
    for (const [key, child] of Object.entries(node)) {
      const dynamic = node.type === 'TemplateLiteral' ? node.expressions.length !== 0 : templateDynamic;
      if (Array.isArray(child)) child.forEach(item => visit(item, node, key, dynamic));
      else if (child && typeof child === 'object') visit(child, node, key, dynamic);
    }
    if (kind) stack.pop();
  }
  visit(ast, null, null);
  entities.sort((a, b) => a.start - b.start || b.end - a.end || (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0));
  matches.sort((a, b) => a.start - b.start || a.end - b.end);
  const rows = entities.map(e => [e.kind, e.start, e.end, e.parent, e.depth, e.name,
    e.flags.async, e.flags.generator, e.flags.static]);
  const selected = new Map();
  for (const index of [0, 1, Math.floor(entities.length / 3), Math.floor(entities.length * 2 / 3), entities.length - 1]) {
    if (entities[index]) selected.set(JSON.stringify(ref(entities[index])), entities[index]);
  }
  for (const match of matches) {
    const entity = entities.find(e => JSON.stringify(ref(e)) === JSON.stringify(match.entity));
    selected.set(JSON.stringify(ref(entity)), entity);
  }
  return { entity_count: entities.length, structure_sha256: hash(JSON.stringify(rows)),
    matches, sample_entities: [...selected.values()] };
}

const manifest = {
  corpus_version: 1,
  generator: { file: 'generate.mjs', sha256: hash(readFileSync(fileURLToPath(import.meta.url))),
    node: process.version, package_lock_sha256: hash(readFileSync(join(root, 'package-lock.json'))),
    packages, oracle: 'Acorn AST; offsets converted from UTF-16 indices to original UTF-8 bytes' },
  cases: [],
};

function add(name, source, needle, provenance, expectedPlaintext = true) {
  const bytes = Buffer.from(source);
  const expected = oracle(source, needle);
  if (Boolean(expected.matches.length) !== expectedPlaintext) {
    throw new Error(`${name}: plaintext expectation disagrees with Acorn`);
  }
  const file = `artifacts/${name}.js`;
  writeFileSync(join(root, file), bytes);
  manifest.cases.push({ name, file, input_bytes: bytes.length, sha256: hash(bytes), needle,
    plaintext_find: expectedPlaintext, source_type: 'script', provenance, expected });
  console.log(`${name}: ${bytes.length} bytes, ${expected.entity_count} entities, ${expected.matches.length} matches`);
}

for (const [packageName, file, license, needle] of [
  ['jquery', 'dist/jquery.min.js', 'LICENSE.txt', 'Syntax error, unrecognized expression:'],
  ['lodash', 'lodash.min.js', 'LICENSE', 'Expected a function'],
  ['vue', 'dist/vue.global.prod.js', 'LICENSE', 'https://vuejs.org/error-reference/#runtime-'],
]) {
  const version = packages[packageName];
  const licenseFile = `licenses/${packageName}-${version}.txt`;
  copyFileSync(join(root, 'node_modules', packageName, license), join(root, licenseFile));
  add(`${packageName}-${version}-production`, readFileSync(join(root, 'node_modules', packageName, file), 'utf8'), needle,
    { kind: 'published-production', package: packageName, version, package_file: file,
      registry: `https://registry.npmjs.org/${packageName}/-/${packageName}-${version}.tgz`,
      license: 'MIT', license_file: licenseFile });
}

const application = readFileSync(join(root, 'application.js'), 'utf8');
const sourceHash = hash(application);
const needle = '/api/login';
const terserOptions = { compress: true, mangle: { toplevel: true }, format: { comments: false } };
const minified = await minify(application, terserOptions);
add('application-terser', minified.code, needle,
  { kind: 'generated-minified', tool: 'terser', version: packages.terser,
    source: 'application.js', source_sha256: sourceHash, options: terserOptions });

const base = { target: 'browser', seed: 20261010, compact: true, sourceMap: false,
  renameGlobals: true, renameProperties: false, debugProtection: false,
  selfDefending: false, disableConsoleOutput: false };
for (const [name, options, plaintext] of [
  ['application-renamed-escaped', { stringArray: false, unicodeEscapeSequence: true }, true],
  ['application-flattened-string-array', { controlFlowFlattening: true, controlFlowFlatteningThreshold: 1,
    deadCodeInjection: true, deadCodeInjectionThreshold: 0.2, stringArray: true,
    stringArrayThreshold: 1, stringArrayEncoding: [], transformObjectKeys: true }, true],
  ['application-rc4', { stringArray: true, stringArrayThreshold: 1,
    stringArrayEncoding: ['rc4'], forceTransformStrings: ['.*'] }, false],
]) {
  const result = Obfuscator.obfuscate(application, { ...base, ...options });
  add(name, result.getObfuscatedCode(), needle,
    { kind: 'generated-obfuscated', tool: 'javascript-obfuscator', version: packages['javascript-obfuscator'],
      source: 'application.js', source_sha256: sourceHash, options: result.getOptions() }, plaintext);
}
copyFileSync(join(root, 'node_modules/javascript-obfuscator/LICENSE.BSD'), join(root, 'licenses/javascript-obfuscator.txt'));
writeFileSync(join(root, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
