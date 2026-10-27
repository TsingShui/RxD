// Structural diff between reconstructed records and the pinned xpanda reference.
// Parses only; never links or executes analyzed code.
const fs = require("node:fs");
const path = require("node:path");
const [parserPath, referenceDir, actualDir, output] = process.argv.slice(2);
if (!parserPath || !referenceDir || !actualDir || !output) {
    throw new Error("usage: node source-diff.cjs PARSER REFERENCE_DIR ACTUAL_DIR REPORT");
}
const parser = require(parserPath);

function summary(file) {
    const ast = parser.parse(fs.readFileSync(file, "utf8"), { sourceType: "module" });
    const classes = [];
    const functions = [];
    const ops = new Map();
    const add = (key) => ops.set(key, (ops.get(key) || 0) + 1);
    const visit = (node) => {
        if (!node || typeof node !== "object") return;
        if (Array.isArray(node)) { for (const item of node) visit(item); return; }
        switch (node.type) {
            case "ClassDeclaration":
            case "ClassExpression":
                classes.push({
                    name: node.id ? node.id.name : null,
                    hasSuper: !!node.superClass,
                    members: node.body.body.map((member) => ({
                        kind: member.type === "ClassMethod" ? member.kind : member.type,
                        name: member.key && (member.key.name ?? member.key.value) || null,
                        static: !!member.static,
                        params: member.params ? member.params.length : null,
                        async: !!member.async,
                        generator: !!member.generator,
                    })),
                });
                break;
            case "FunctionDeclaration":
            case "FunctionExpression":
            case "ArrowFunctionExpression":
                functions.push({
                    type: node.type,
                    name: node.id ? node.id.name : null,
                    params: node.params.length,
                    async: !!node.async,
                    generator: !!node.generator,
                });
                break;
            case "MemberExpression": {
                const property = !node.computed && node.property.type === "Identifier"
                    ? node.property.name
                    : node.computed && node.property.type === "StringLiteral"
                        ? node.property.value
                        : "[]";
                add("member:" + property);
                break;
            }
            case "ObjectProperty": {
                if (node.key && node.key.type === "StringLiteral" && node.computed) {
                    add("key:" + node.key.value);
                    for (const value of [node.value]) visit(value);
                    return;
                }
                break;
            }
            case "CallExpression": {
                const callee = node.callee;
                const name = callee.type === "Identifier"
                    ? callee.name
                    : callee.type === "MemberExpression"
                        ? (!callee.computed && callee.property.type === "Identifier" ? callee.property.name : "[]")
                        : callee.type;
                add("call:" + name);
                break;
            }
            case "StringLiteral": add("str:" + node.value); break;
            case "Identifier": if (node.name.startsWith("__abc")) add("member:" + node.name); break;
            case "NumericLiteral": add("num:" + String(node.value)); break;
            case "BinaryExpression":
            case "LogicalExpression": add("bin:" + node.operator); break;
            case "UnaryExpression": add("un:" + node.operator); break;
            default: break;
        }
        for (const key of Object.keys(node)) {
            if (["loc", "start", "end", "leadingComments", "trailingComments"].includes(key)) continue;
            visit(node[key]);
        }
    };
    visit(ast.program);
    return { classes, functions, ops: [...ops.entries()].sort() };
}

const SYNTHETIC = new Set([
    "apply", "defineProperty", "has", "fill", "parent", "slots", "hole", "newTarget",
    "call", "splice", "slice", "keys", "iterator", "Symbol", "Object", "Reflect",
    "WeakSet", "add", "constructor", "prototype", "next", "value", "arguments",
    "__abc_context", "__abc_env", "__abc_this", "__abc_await", "__abc_resume",
    "env", "length", "number", "acc", "undefined",
    "sub-class must call super before use 'this'",
    "super() forbidden re-bind 'this'",
]);
const isSynthetic = (key) => {
    const [kind, name] = key.split(":");
    if (kind === "num" || kind === "bin" || kind === "un") return true;
    return (kind === "member" || kind === "call") && (SYNTHETIC.has(name) || name.startsWith("__abc"));
};
const refFiles = fs.readdirSync(referenceDir).filter((name) => name.endsWith(".js")).sort();
const report = { records: 0, missing: [], parseErrors: [], structural: [], totals: { opDelta: 0, sourceDelta: 0 }, opDeltas: new Map(), sourceDeltas: new Map(), missingKeys: new Map(), extraKeys: new Map() };
for (const name of refFiles) {
    const stem = name.slice(0, -3);
    const refPath = path.join(referenceDir, name);
    const actPath = path.join(actualDir, name);
    report.records++;
    if (!fs.existsSync(actPath)) { report.missing.push(stem); continue; }
    let ref, act;
    try { ref = summary(refPath); act = summary(actPath); }
    catch (error) { report.parseErrors.push({ stem, error: String(error) }); continue; }
    const refClasses = ref.classes.map((item) => `${item.name}/${item.hasSuper}`).sort();
    const actClasses = act.classes.map((item) => `${item.name}/${item.hasSuper}`).sort();
    const refMembers = ref.classes.flatMap((item) => item.members.map((m) => `${m.kind}/${m.static}/${m.name}`)).sort();
    const actMembers = act.classes.flatMap((item) => item.members.map((m) => `${m.kind}/${m.static}/${m.name}`)).sort();
    const refFns = ref.functions.filter((item) => item.name && !item.name.startsWith("_abc_"));
    const actFns = act.functions.filter((item) => item.name && !item.name.startsWith("_abc_"));
    const structural = [];
    for (const [label, left, right] of [["class", refClasses, actClasses], ["member", refMembers, actMembers]]) {
        const leftSet = new Set(left), rightSet = new Set(right);
        for (const value of leftSet) if (!rightSet.has(value)) structural.push({ kind: label + "-missing", value });
        for (const value of rightSet) if (!leftSet.has(value)) structural.push({ kind: label + "-extra", value });
    }
    const refOps = new Map(ref.ops), actOps = new Map(act.ops);
    const missingKeys = [];
    const extraKeys = [];
    for (const key of new Set([...refOps.keys(), ...actOps.keys()])) {
        if (isSynthetic(key)) continue;
        const inRef = (refOps.get(key) || 0) > 0;
        const inAct = (actOps.get(key) || 0) > 0;
        if (inRef && !inAct) missingKeys.push(key);
        if (!inRef && inAct) extraKeys.push(key);
    }
    for (const key of missingKeys) report.missingKeys.set(key, (report.missingKeys.get(key) || 0) + 1);
    for (const key of extraKeys) report.extraKeys.set(key, (report.extraKeys.get(key) || 0) + 1);
    const deltas = [];
    for (const key of new Set([...refOps.keys(), ...actOps.keys()])) {
        const delta = (refOps.get(key) || 0) - (actOps.get(key) || 0);
        if (delta === 0) continue;
        deltas.push({ key, delta });
        report.totals.opDelta += Math.abs(delta);
        report.opDeltas.set(key, (report.opDeltas.get(key) || 0) + delta);
        if (!isSynthetic(key)) {
            report.totals.sourceDelta += Math.abs(delta);
            report.sourceDeltas.set(key, (report.sourceDeltas.get(key) || 0) + delta);
        }
    }
    deltas.sort((a, b) => Math.abs(b.delta) - Math.abs(a.delta));
    report.structural.push({ stem, missingKeys: missingKeys.slice(0, 12), extraKeys: extraKeys.slice(0, 12), refFunctions: refFns.length, actFunctions: actFns.length, refClasses: ref.classes.length, actClasses: act.classes.length, structural, opDelta: deltas.reduce((n, d) => n + Math.abs(d.delta), 0), sourceDelta: deltas.filter((d) => !isSynthetic(d.key)).reduce((n, d) => n + Math.abs(d.delta), 0), top: deltas.filter((d) => !isSynthetic(d.key)).sort((a, b) => Math.abs(b.delta) - Math.abs(a.delta)).slice(0, 8) });
}
report.recordsWithStructural = report.structural.filter((item) => item.structural.length > 0).length;
report.recordsWithOpDelta = report.structural.filter((item) => item.opDelta > 0).length;
report.topOpDeltas = [...report.opDeltas.entries()].sort((a, b) => Math.abs(b[1]) - Math.abs(a[1])).slice(0, 40);
report.topSourceDeltas = [...report.sourceDeltas.entries()].sort((a, b) => Math.abs(b[1]) - Math.abs(a[1])).slice(0, 60);
report.topMissingKeys = [...report.missingKeys.entries()].sort((a, b) => b[1] - a[1]).slice(0, 40);
report.topExtraKeys = [...report.extraKeys.entries()].sort((a, b) => b[1] - a[1]).slice(0, 40);
report.recordsWithMissingKeys = report.structural.filter((item) => item.missingKeys.length > 0).length;
report.largestSourceRecords = report.structural.filter((item) => item.sourceDelta > 0).sort((a, b) => b.sourceDelta - a.sourceDelta).slice(0, 20).map((item) => ({ stem: item.stem, sourceDelta: item.sourceDelta, top: item.top }));
const serializable = { ...report, opDeltas: undefined, structural: report.structural.slice(0, 20), sourceDeltas: undefined, missingKeys: undefined, extraKeys: undefined, totalSourceDelta: report.totals.sourceDelta };
fs.writeFileSync(output, JSON.stringify(serializable, null, 2) + "\n", { flag: "wx" });
console.log(JSON.stringify({ records: report.records, missing: report.missing.length, parseErrors: report.parseErrors.length, structuralRecords: report.recordsWithStructural, totalSourceDelta: report.totals.sourceDelta, recordsWithMissingKeys: report.recordsWithMissingKeys, topMissing: report.topMissingKeys.slice(0, 20), topExtra: report.topExtraKeys.slice(0, 10) }, null, 2));
