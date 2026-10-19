// Independently map pinned literal/reference anchors to local syntax. Never execute inputs.
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'acorn';

const root = dirname(fileURLToPath(import.meta.url));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const manifest = JSON.parse(readFileSync(join(root, 'manifest.json')));
const refs = JSON.parse(readFileSync(join(root, 'refs-expectations.json')));
const acorn = JSON.parse(readFileSync(join(root, 'node_modules/acorn/package.json'))).version;
if (acorn !== manifest.generator.packages.acorn) throw new Error('Unpinned Acorn version');
const kinds = {
  VariableDeclaration: 'variable_declaration', VariableDeclarator: 'variable_declarator',
  AssignmentExpression: 'assignment_expression', UpdateExpression: 'update_expression',
  PropertyDefinition: 'class_field', ImportDeclaration: 'import_declaration',
  ExportNamedDeclaration: 'export_named_declaration', ExportDefaultDeclaration: 'export_default_declaration',
  ExportAllDeclaration: 'export_all_declaration', ExpressionStatement: 'expression_statement',
  ReturnStatement: 'return_statement', ThrowStatement: 'throw_statement', IfStatement: 'if_statement',
  SwitchStatement: 'switch_statement', SwitchCase: 'switch_case', ForStatement: 'for_statement',
  ForInStatement: 'for_in_statement', ForOfStatement: 'for_of_statement', WhileStatement: 'while_statement',
  DoWhileStatement: 'do_while_statement', TryStatement: 'try_statement', WithStatement: 'with_statement',
  LabeledStatement: 'labeled_statement', BreakStatement: 'break_statement', ContinueStatement: 'continue_statement',
  DebuggerStatement: 'debugger_statement', EmptyStatement: 'empty_statement',
};
const boundaries = new Set(['Program', 'FunctionDeclaration', 'FunctionExpression',
  'ArrowFunctionExpression', 'ClassDeclaration', 'ClassExpression', 'MethodDefinition']);
const method = node => node.type === 'Property' && (node.method || node.kind !== 'init');

const cases = manifest.cases.map(input => {
  const bytes = readFileSync(join(root, input.file));
  if (hash(bytes) !== input.sha256) throw new Error(`Changed corpus ${input.name}`);
  const source = bytes.toString('utf8');
  const offsets = new Map();
  const byte = offset => {
    if (!offsets.has(offset)) offsets.set(offset, Buffer.byteLength(source.slice(0, offset)));
    return offsets.get(offset);
  };
  const range = node => ({ start: byte(node.start), end: byte(node.end) });
  const contextKey = context => context && { kind: context.kind, start: context.start, end: context.end };
  const anchors = new Map();
  const ast = parse(source, { ecmaVersion: 'latest', sourceType: 'script' });
  function walk(node, parent = null, field = null, context = null) {
    if (!node || typeof node.type !== 'string') return;
    if (boundaries.has(node.type) || method(node)) context = null;
    else {
      let kind = kinds[node.type];
      if (node.type === 'Property' && parent?.type !== 'ObjectPattern') kind = 'object_property';
      if (field === 'params' && ['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression'].includes(parent?.type))
        kind = node.type === 'RestElement' ? 'rest_parameter' : 'parameter';
      if (parent?.type === 'CatchClause' && field === 'param') kind = 'catch_parameter';
      // Acorn represents directives as expression statements; Oxc has a separate Directive node.
      if (node.type === 'ExpressionStatement' && node.directive !== undefined) kind = null;
      if (kind) context = { kind, ...range(node), parent: contextKey(context) };
    }
    if (['Literal', 'TemplateElement', 'Identifier'].includes(node.type)) {
      const span = range(node);
      anchors.set(`${span.start}:${span.end}`, context);
    }
    for (const [key, child] of Object.entries(node)) {
      if (Array.isArray(child)) child.forEach(item => walk(item, node, key, context));
      else if (child && typeof child === 'object') walk(child, node, key, context);
    }
  }
  walk(ast);
  const lookup = span => {
    const key = `${span.start}:${span.end}`;
    if (!anchors.has(key)) throw new Error(`No Acorn anchor ${input.name} ${key}`);
    return { ...span, context: anchors.get(key) };
  };
  const reference = refs.cases.find(query => query.name === input.name);
  if (!reference || reference.source_id !== input.sha256) throw new Error('Invalid independent refs input');
  return { name: input.name, file: input.file, source_id: input.sha256,
    matches: input.expected.matches.map(({ start, end }) => lookup({ start, end })),
    selected: lookup(reference.declaration), references: reference.references.map(({ start, end }) => lookup({ start, end })) };
});
const output = { contexts_version: 1,
  generator: { file: 'generate-contexts.mjs', sha256: hash(readFileSync(fileURLToPath(import.meta.url))),
    acorn, node: process.version, refs_sha256: hash(readFileSync(join(root, 'refs-expectations.json'))),
    oracle: 'Acorn local syntax ancestor ranges and immediate context parents; function/class/method boundaries reset contexts; anchors from pinned literal and independent restricted refs expectations' }, cases };
writeFileSync(join(root, 'contexts-expectations.json'), JSON.stringify(output, null, 2) + '\n');
console.log(`Wrote ${cases.length} independent context queries.`);
