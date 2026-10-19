// Independent refs expectations for simple single-parameter functions. Parse inputs, never execute them.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'acorn';

const root = dirname(fileURLToPath(import.meta.url));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const manifest = JSON.parse(readFileSync(join(root, 'manifest.json')));
const acorn = JSON.parse(readFileSync(join(root, 'node_modules/acorn/package.json'))).version;
if (acorn !== manifest.generator.packages.acorn) throw new Error('Acorn version differs from pinned corpus');
const allowed = new Set(['Identifier', 'Literal', 'BinaryExpression', 'LogicalExpression',
  'UnaryExpression', 'MemberExpression', 'CallExpression', 'NewExpression',
  'ConditionalExpression', 'ArrayExpression', 'ChainExpression', 'TemplateLiteral',
  'TemplateElement', 'SpreadElement', 'ThisExpression']);

function walk(node, callback, parent = null, key = null) {
  if (!node || typeof node.type !== 'string') return;
  callback(node, parent, key);
  for (const [name, child] of Object.entries(node)) {
    if (Array.isArray(child)) child.forEach(item => walk(item, callback, node, name));
    else if (child && typeof child === 'object') walk(child, callback, node, name);
  }
}

const cases = manifest.cases.map(input => {
  const bytes = readFileSync(join(root, input.file));
  if (hash(bytes) !== input.sha256) throw new Error(`Changed corpus: ${input.name}`);
  const source = bytes.toString('utf8');
  const byte = offset => Buffer.byteLength(source.slice(0, offset));
  const ast = parse(source, { ecmaVersion: 'latest', sourceType: 'script' });
  let dynamic = false;
  let selected = null;
  walk(ast, node => {
    if (node.type === 'WithStatement' || (node.type === 'CallExpression'
      && !node.optional && node.callee.type === 'Identifier' && node.callee.name === 'eval')) dynamic = true;
    if (selected || !['FunctionDeclaration', 'ArrowFunctionExpression'].includes(node.type)
      || node.params.length !== 1 || node.params[0].type !== 'Identifier'
      || node.params[0].name === node.id?.name) return;
    let expression = node.body;
    if (node.body.type === 'BlockStatement') {
      if (node.body.body.length !== 1) return;
      const statement = node.body.body[0];
      if (!['ReturnStatement', 'ThrowStatement'].includes(statement.type) || !statement.argument) return;
      expression = statement.argument;
    }
    const parameter = node.params[0];
    let valid = true;
    const references = [];
    walk(expression, (expression, parent, key) => {
      if (!allowed.has(expression.type) || (expression.type === 'UnaryExpression' && expression.operator === 'delete')) valid = false;
      if (expression.type !== 'Identifier' || expression.name !== parameter.name) return;
      if (parent?.type === 'MemberExpression' && key === 'property' && !parent.computed) return;
      references.push({ start: byte(expression.start), end: byte(expression.end),
        read: true, write: false, member_write: false });
    });
    if (valid && references.length) selected = {
      identifier: parameter.name, byte: byte(parameter.start),
      declaration: { start: byte(parameter.start), end: byte(parameter.end) },
      entity: { kind: node.type === 'FunctionDeclaration' ? 'function_declaration' : 'arrow_function',
        start: byte(node.start), end: byte(node.end) },
      references: references.sort((a, b) => a.start - b.start || a.end - b.end),
    };
  });
  if (!selected) throw new Error(`No independent simple-function query: ${input.name}`);
  return { name: input.name, file: input.file, source_id: input.sha256,
    resolution: dynamic ? 'dynamic' : 'resolved', ...selected,
    references: dynamic ? [] : selected.references };
});
const output = {
  refs_version: 1,
  generator: { file: 'generate-refs.mjs', sha256: hash(readFileSync(fileURLToPath(import.meta.url))),
    acorn,
    oracle: 'Acorn AST: one-parameter function/arrow with one return/throw or expression body and no nested bindings or assignments; literal/static-property names excluded; UTF-8 byte spans' },
  cases,
};
writeFileSync(join(root, 'refs-expectations.json'), JSON.stringify(output, null, 2) + '\n');
console.log(`Wrote ${cases.length} independent refs queries.`);
