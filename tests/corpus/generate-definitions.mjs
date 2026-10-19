// Independent source-declaration oracle. Parse pinned inputs; never execute them.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'acorn';

const root = dirname(fileURLToPath(import.meta.url));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const manifest = JSON.parse(readFileSync(join(root, 'manifest.json')));
const acorn = JSON.parse(readFileSync(join(root, 'node_modules/acorn/package.json'))).version;
if (acorn !== manifest.generator.packages.acorn) throw new Error('Unpinned Acorn version');

function walk(node, callback, ancestors = []) {
  if (!node || typeof node.type !== 'string') return;
  callback(node, ancestors);
  const parents = [...ancestors, node];
  for (const child of Object.values(node)) {
    if (Array.isArray(child)) child.forEach(item => walk(item, callback, parents));
    else if (child && typeof child === 'object') walk(child, callback, parents);
  }
}

function boundNames(pattern, callback) {
  if (!pattern) return;
  switch (pattern.type) {
    case 'Identifier': callback(pattern.name); break;
    case 'AssignmentPattern': boundNames(pattern.left, callback); break;
    case 'RestElement': boundNames(pattern.argument, callback); break;
    case 'ArrayPattern': pattern.elements.forEach(element => boundNames(element, callback)); break;
    case 'ObjectPattern': pattern.properties.forEach(property =>
      boundNames(property.type === 'RestElement' ? property.argument : property.value, callback)); break;
    default: throw new Error(`Unexpected binding pattern ${pattern.type}`);
  }
}

const cases = manifest.cases.map(input => {
  const bytes = readFileSync(join(root, input.file));
  if (hash(bytes) !== input.sha256) throw new Error(`Changed corpus ${input.name}`);
  const source = bytes.toString('utf8');
  const byte = offset => Buffer.byteLength(source.slice(0, offset));
  const range = node => ({ start: byte(node.start), end: byte(node.end) });
  const ast = parse(source, { ecmaVersion: 'latest', sourceType: 'script' });
  const counts = new Map();
  const count = name => counts.set(name, (counts.get(name) ?? 0) + 1);
  walk(ast, node => {
    if (node.type === 'VariableDeclarator') boundNames(node.id, count);
    if (['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression'].includes(node.type)) {
      boundNames(node.id, count);
      node.params.forEach(parameter => boundNames(parameter, count));
    }
    if (['ClassDeclaration', 'ClassExpression'].includes(node.type)) boundNames(node.id, count);
    if (node.type === 'CatchClause') boundNames(node.param, count);
    if (node.type === 'ImportDeclaration') node.specifiers.forEach(specifier => boundNames(specifier.local, count));
  });
  let selected = null;
  let best = -1;
  walk(ast, (node, ancestors) => {
    // A name bound only once anywhere in the file cannot select a redeclaration
    // or shadowed parameter; no general scope solver is claimed by this oracle.
    if (node.type !== 'VariableDeclarator' || node.id.type !== 'Identifier'
      || !node.init || counts.get(node.id.name) !== 1) return;
    const owner = ancestors.findLast(parent => ['FunctionDeclaration', 'FunctionExpression',
      'ArrowFunctionExpression', 'ClassDeclaration', 'ClassExpression', 'Program'].includes(parent.type));
    const declaration = range(node);
    if (declaration.end - declaration.start < 32 || declaration.end - declaration.start > 2048) return;
    const score = (byte(owner.end) - byte(owner.start)) / (declaration.end - declaration.start);
    if (score <= best) return;
    best = score;
    selected = { binding_name: node.id.name, identifier: range(node.id),
      declaration: { kind: 'variable_declarator', ...declaration,
        variable_kind: ancestors.at(-1).kind, initializer: range(node.init) } };
  });
  if (!selected) throw new Error(`No uniquely bound variable query: ${input.name}`);
  return { name: input.name, file: input.file, source_id: input.sha256, ...selected };
});
const output = { definitions_version: 1,
  generator: { file: 'generate-definitions.mjs', sha256: hash(readFileSync(fileURLToPath(import.meta.url))),
    acorn, node: process.version,
    selection: 'Among initialized declarators of 32-2048 bytes, maximize enclosing-structure/declarator size ratio; selected examples demonstrate extraction savings, not average workload savings',
    oracle: 'Acorn AST: initialized variable with an Identifier pattern whose bound name occurs exactly once across all input declarations/parameters; canonical identifier, declarator, kind and initializer UTF-8 byte ranges' },
  cases };
writeFileSync(join(root, 'definitions-expectations.json'), JSON.stringify(output, null, 2) + '\n');
console.log(`Wrote ${cases.length} independent declaration queries.`);
