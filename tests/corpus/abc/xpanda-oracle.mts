import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";
import { gzipSync } from "node:zlib";

const REVISION = "8f27d3ce8b63581c7eb5203bbfdfbb92e28cfccf";
const INPUT_SHA256 = "a2046b75ebd368e0599a50b2b5e5c6dab8070007364af5a09d67d267582c197c";
const { values } = parseArgs({ options: {
    xpanda: { type: "string" },
    input: { type: "string" },
    output: { type: "string" },
    stage: { type: "string", default: "metadata" },
    record: { type: "string" },
} });
assert(["metadata", "decode", "full"].includes(values.stage!), "stage must be metadata, decode or full");
assert(values.xpanda && values.input && values.output,
    "usage: tsx xpanda-oracle.mts --xpanda CHECKOUT --input modules.abc --output NEW_DIR");
const root = path.resolve(values.xpanda);
const output = path.resolve(values.output);
const revision = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
assert.equal(revision, REVISION, "the reference revision must match the pinned baseline");
assert.equal(execFileSync("git", ["-C", root, "status", "--porcelain", "--untracked-files=no"],
    { encoding: "utf8" }).trim(), "", "the reference checkout must have no tracked modifications");
const digest = (value: Uint8Array | string) => createHash("sha256").update(value).digest("hex");
const bytes = new Uint8Array(fs.readFileSync(values.input));
assert.equal(digest(bytes), INPUT_SHA256, "the input must match the pinned ClashBox ABC");
assert.equal(bytes.length, 7_203_484);
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.mkdirSync(output);
fs.mkdirSync(path.join(output, "records"));

const load = (relative: string) => import(pathToFileURL(path.join(root, relative)).href);
const { PandaFile } = await load("packages/xpanda-core/src/file/PandaFile.ts");
const { PandaDecompiler } = await load("packages/xpanda-core/src/PandaDecompiler.ts");
const { PandaLexical } = await load("packages/xpanda-core/src/disasm/PandaLexical.ts");
const { PandaInstructionIMM, PandaInstructionVReg, PandaInstructionID } =
    await load("packages/xpanda-core/src/disasm/PandaInstruction.ts");
const { OPCODE_BY_CODE } = await load("packages/xpanda-core/src/disasm/opcodes.ts");
const start = performance.now();
const file = new PandaFile(bytes);
const parsed = performance.now();
const raw = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const classOffsets = Array.from({ length: raw.getUint32(28, true) },
    (_, index) => raw.getUint32(raw.getUint32(32, true) + index * 4, true));
const indexed = new Map<number, any>();
for (const record of file.getAllClasses()) {
    if (record.isValid()) indexed.set(record.getOffset(), record);
}
assert.equal(indexed.size, classOffsets.length);
assert.equal(new Set(classOffsets).size, classOffsets.length);
const allRecords = classOffsets.map(offset => {
    assert(indexed.has(offset), `global record missing from region indexes: ${offset}`);
    return indexed.get(offset);
});
const records = values.record ? allRecords.filter(record => `record:0x${record.getOffset().toString(16)}` === values.record) : allRecords;
assert(records.length > 0, "selected record not found");
const originalConsole = { log: console.log, warn: console.warn, error: console.error };
const logFd = fs.openSync(path.join(output, "diagnostics.jsonl"), "wx");
let activeRecord: number | null = null;
let diagnosticCount = 0;
const log = (level: string, args: unknown[]) => {
    diagnosticCount++;
    fs.writeSync(logFd, JSON.stringify({ record: activeRecord, level,
        message: args.map(value => String(value)).join(" ") }) + "\n");
};
console.log = (...args) => log("log", args);
console.warn = (...args) => log("warn", args);
console.error = (...args) => log("error", args);
const metadata: any[] = [];
const errors: any[] = [];
const histogram = new Map<string, number>();
const unknownKinds = new Map<string, number>();
let methodCount = 0;
let codeBytes = 0;
let instructionCount = 0;
let tryCount = 0;
let sourceCount = 0;
let lexicalMethodCount = 0;

function parameter(value: any, method: any): unknown {
    if (value instanceof PandaInstructionIMM) {
        const number = value.imm;
        return { kind: "imm", value: Object.is(number, -0) ? "-0"
            : Number.isFinite(number) ? number : String(number) };
    }
    if (value instanceof PandaInstructionVReg) {
        return { kind: "reg", index: value.index
            + (value.type === "PARAM" ? method.methodCode.numVregs : 0) };
    }
    if (value instanceof PandaInstructionID) {
        return { kind: "id", type: value.type, offset: value.obj?.getOffset() ?? null,
            ...(value.type === "STRING" ? { value: value.obj?.content } : {}) };
    }
    throw new Error(`unrecognized reference operand: ${value?.constructor?.name}`);
}

try {
    for (const record of records) {
        activeRecord = record.getOffset();
        const stem = activeRecord.toString(16);
        const methods = [...record.pandaMethods.values()] as any[];
        assert.equal(methods.length, record.numMethods, "reference name-keyed map lost a method");
        const meta: any = {
            offset: activeRecord,
            name: record.name.content,
            fields: record.numFields,
            declaredMethods: record.numMethods,
            hasMain: record.pandaMethods.has("func_main_0"),
            hasModule: record.pandaClassModuleData !== null,
            methods: [],
        };
        const traces: any[] = [];
        for (const method of methods) {
            methodCount++;
            const code = method.methodCode;
            const item: any = { offset: method.getOffset(), name: method.name.content,
                flags: method.accessFlag, code: code?.getOffset() ?? null };
            meta.methods.push(item);
            if (!code) continue;
            codeBytes += code.codeSize;
            tryCount += code.triesSize;
            const trace: any = {
                ...item,
                vregs: code.numVregs,
                args: code.numArgs,
                codeSize: code.codeSize,
                codeHex: Buffer.from(code.instructions).toString("hex"),
                tries: (code.pandaMethodTryBlocks ?? []).map((block: any) => ({
                    start: block.startPC, length: block.length,
                    catches: block.pandaMethodCatchBlocks.map((handler: any) => ({
                        type: handler.typeIdx, pc: handler.handlerPC, size: handler.codeSize,
                    })),
                })),
            };
            if (values.stage === "metadata") { traces.push(trace); continue; }
            try {
                const cfg = method.disAssemble();
                const byPC = new Map<number, any>();
                for (const block of cfg.pandaIRBasicBlocks) {
                    for (const instruction of block.pandaInstructions) {
                        if (byPC.has(instruction.pc)) {
                            assert.equal(byPC.get(instruction.pc).opcode.code, instruction.opcode.code);
                        } else byPC.set(instruction.pc, instruction);
                    }
                }
                const instructions = [...byPC.values()].sort((left, right) => left.pc - right.pc);
                let end = 0;
                trace.instructions = instructions.map(instruction => {
                    assert.equal(instruction.pc, end, "reference CFG omitted instruction bytes");
                    const size = instruction.instructionSize();
                    end += size;
                    const name = instruction.opcode.name;
                    histogram.set(name, (histogram.get(name) ?? 0) + 1);
                    if (instruction.kind === "unknown") {
                        unknownKinds.set(name, (unknownKinds.get(name) ?? 0) + 1);
                    }
                    return { pc: instruction.pc, size, opcode: instruction.opcode.code,
                        name, format: instruction.opcode.format, kind: instruction.kind,
                        params: instruction.params.map((param: any) => parameter(param, method)) };
                });
                assert.equal(end, code.codeSize, "reference CFG did not consume the full code item");
                instructionCount += instructions.length;
                item.instructions = instructions.length;
                item.traceSha256 = digest(JSON.stringify(trace));
            } catch (error) {
                trace.error = String(error);
                item.error = String(error);
                errors.push({ record: activeRecord, method: method.getOffset(), stage: "decode", error: String(error) });
            }
            traces.push(trace);
        }
        const traceBytes = gzipSync(JSON.stringify(traces));
        fs.writeFileSync(path.join(output, "records", `${stem}.methods.json.gz`), traceBytes);
        meta.traceFileSha256 = digest(traceBytes);
        if (values.stage === "full") {
        try {
            const lexical = new PandaLexical(record);
            const absent = methods.filter(method => !lexical.has(method));
            meta.methodsOutsideReferenceLexicalTraversal = absent.map(method => method.getOffset());
            lexicalMethodCount += methods.length - absent.length;
            const decompiler = new PandaDecompiler();
            const ast = decompiler.decompileClassAst(record);
            if (ast.length === 0) {
                meta.noSourceReason = meta.hasMain ? "reference returned empty AST despite module entry" : "no func_main_0";
            } else {
                fs.writeFileSync(path.join(output, "records", `${stem}.ast.json.gz`), gzipSync(ast));
                const source = decompiler.simplifyInProcess(ast);
                meta.sourceBytes = Buffer.byteLength(source);
                meta.sourceSha256 = digest(source);
                fs.writeFileSync(path.join(output, "records", `${stem}.js`), source);
                if (source.length > 0) sourceCount++;
                else meta.noSourceReason = "reference simplify returned empty output";
            }
        } catch (error) {
            meta.error = String(error);
            errors.push({ record: activeRecord, stage: "source", error: String(error) });
        }
        }
        metadata.push(meta);
    }
} finally {
    Object.assign(console, originalConsole);
    fs.closeSync(logFd);
}
const opcodeTable = [...OPCODE_BY_CODE.values()]
    .sort((left: any, right: any) => left.code - right.code);
fs.writeFileSync(path.join(output, "opcodes.json"), JSON.stringify(opcodeTable, null, 2) + "\n");
const summary = {
    stage: values.stage, selectedRecord: values.record ?? null,
    input: path.resolve(values.input), inputBytes: bytes.length, inputSha256: digest(bytes),
    reference: { repository: "https://github.com/asmjmp0/xpanda", revision,
        packageVersion: JSON.parse(fs.readFileSync(path.join(root, "packages/xpanda-core/package.json"), "utf8")).version,
        nodeVersion: process.version, isaSha256: digest(fs.readFileSync(path.join(root, "packages/xpanda-core/resources/isa.yaml"))) },
    version: file.header.versionString(), rawChecksum: raw.getUint32(8, true),
    referenceChecksum: file.header.checksum,
    classes: records.length, regionClassesIncludingPrimitiveIds: file.getAllClasses().length,
    methods: methodCount, codeBytes, instructions: instructionCount, tries: tryCount,
    sourceFiles: sourceCount, methodsInReferenceLexicalTraversal: lexicalMethodCount,
    errors, diagnosticCount, unknownKinds: Object.fromEntries([...unknownKinds].sort()),
    opcodeHistogram: Object.fromEntries([...histogram].sort()),
    parseMs: parsed - start, totalMs: performance.now() - start,
    noSource: metadata.filter(record => record.noSourceReason)
        .map(({ offset, name, declaredMethods, noSourceReason }) => ({ offset, name, declaredMethods, noSourceReason })),
};
fs.writeFileSync(path.join(output, "records.json"), JSON.stringify(metadata, null, 2) + "\n");
fs.writeFileSync(path.join(output, "summary.json"), JSON.stringify(summary, null, 2) + "\n");
console.log(JSON.stringify(summary, null, 2));
if (errors.length > 0) process.exitCode = 1;
