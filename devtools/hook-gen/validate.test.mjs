// Tests the real offline validation command; supplied programs are only parsed/type checked.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

const validator = fileURLToPath(new URL("./validate.mjs", import.meta.url));

function validate(mode, source, suffix) {
    const temporary = mkdtempSync(join(tmpdir(), "rxd-hook-validator-test-"));
    try {
        const file = join(temporary, `artifact.${suffix}`);
        writeFileSync(file, source);
        return spawnSync(process.execPath, [validator, mode, file], { encoding: "utf8" });
    } finally {
        rmSync(temporary, { recursive: true, force: true });
    }
}

// Deliberately invalid syntax is rejected; the valid source would throw if executed.
test("JS command parses source without executing it and rejects invalid syntax", () => {
    const valid = validate("js", 'throw new Error("must never execute");', "js");
    assert.equal(valid.status, 0, valid.stderr);
    const invalid = validate("js", "const = ;", "js");
    assert.equal(invalid.status, 1);
});

test("strict TS command checks actual Gum and Java bridge declarations", () => {
    const source = `
import Java from "frida-java-bridge";
Java.perform(() => { Java.use("java.lang.String"); });
Interceptor.attach(ptr("0x1000"), {
  onEnter(this: InvocationContext, args: InvocationArguments) {
    const first: NativePointer = args[0];
    console.log(first);
  },
  onLeave(this: InvocationContext, result: InvocationReturnValue) { console.log(result); }
});
`;
    const valid = validate("ts", source, "ts");
    assert.equal(valid.status, 0, `${valid.stdout}${valid.stderr}`);
    const invalid = validate("ts", source.replace('ptr("0x1000")', "123"), "ts");
    assert.equal(invalid.status, 1, "Gum API type mismatch must fail");
    assert.match(invalid.stderr, /TS2345/u);
});

test("extension command checks actual MV3 declarations and rejects isolated world", () => {
    const temporary = mkdtempSync(join(tmpdir(), "rxd-hook-extension-test-"));
    const manifest = {
        manifest_version: 3, name: "rxd offline validation fixture", version: "1.0.0",
        minimum_chrome_version: "111",
        content_scripts: [{ matches: ["https://example.com/*"], js: ["hook.js"], world: "MAIN", run_at: "document_start", all_frames: false }],
    };
    try {
        writeFileSync(join(temporary, "hook.js"), 'throw new Error("must never execute");');
        writeFileSync(join(temporary, "manifest.json"), JSON.stringify(manifest));
        const valid = spawnSync(process.execPath, [validator, "extension", temporary], { encoding: "utf8" });
        assert.equal(valid.status, 0, `${valid.stdout}${valid.stderr}`);
        manifest.content_scripts[0].world = "ISOLATED";
        writeFileSync(join(temporary, "manifest.json"), JSON.stringify(manifest));
        const invalid = spawnSync(process.execPath, [validator, "extension", temporary], { encoding: "utf8" });
        assert.equal(invalid.status, 1);
    } finally {
        rmSync(temporary, { recursive: true, force: true });
    }
});
