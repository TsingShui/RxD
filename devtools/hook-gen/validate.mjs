#!/usr/bin/env node
// Offline artifact checking only. Never import, eval or execute the supplied source.
import assert from "node:assert/strict";
import { lstatSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { parse } from "acorn";

const toolRoot = dirname(fileURLToPath(import.meta.url));
const modules = join(toolRoot, "node_modules");

function regularFile(path) {
    assert(lstatSync(path).isFile(), `not a regular local file: ${path}`);
    return readFileSync(path, "utf8");
}

function checkJavaScript(paths, sourceType = "module") {
    const source = paths.map(regularFile).join("\n");
    parse(source, { ecmaVersion: 2022, sourceType });
}

function checkTypeScript(paths, environment = "frida") {
    const temporary = mkdtempSync(join(tmpdir(), "rxd-hook-types-"));
    try {
        const source = paths.map(regularFile).join("\n");
        const file = join(temporary, "artifact.ts");
        writeFileSync(file, source);
        const config = {
            compilerOptions: {
                strict: true,
                noEmit: true,
                skipLibCheck: false,
                target: "ES2020",
                module: "ESNext",
                moduleResolution: "node",
                // Gum has no Console declaration. Use the pinned real Node declarations,
                // not DOM (whose File/Worker conflict with Gum), or substitute stubs.
                lib: environment === "frida" ? ["ES2020"] : ["ES2020", "DOM"],
                types: environment === "frida" ? ["frida-gum", "node"] : ["chrome"],
                typeRoots: [join(modules, "@types")],
                baseUrl: toolRoot,
                paths: { "frida-java-bridge": [join(modules, "frida-java-bridge", "index.d.ts")] },
            },
            files: [file],
        };
        const configPath = join(temporary, "tsconfig.json");
        writeFileSync(configPath, JSON.stringify(config));
        const result = spawnSync(process.execPath, [join(modules, "typescript", "bin", "tsc"), "--project", configPath, "--pretty", "false"], {
            encoding: "utf8", timeout: 120_000, maxBuffer: 4 * 1024 * 1024,
        });
        assert.ifError(result.error);
        assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
    } finally {
        rmSync(temporary, { recursive: true, force: true });
    }
}

function onlyKeys(value, allowed, label) {
    assert(value !== null && typeof value === "object" && !Array.isArray(value), `${label} must be an object`);
    for (const key of Object.keys(value)) assert(allowed.includes(key), `unrequested ${label} key: ${key}`);
}

function validMatch(pattern) {
    if (pattern === "<all_urls>") return true; // Explicit user selection, never an implicit default.
    if (typeof pattern !== "string" || /[\s\u0000-\u001f\u007f]/u.test(pattern)) return false;
    const match = /^(\*|http|https|file|ftp):\/\/([^/]*)(\/.*)$/u.exec(pattern);
    if (!match) return false;
    const [, scheme, host] = match;
    if (scheme === "file") return host === "";
    return host === "*" || /^(?:\*\.)?(?:[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)*$/u.test(host);
}

function checkExtension(directory) {
    assert(lstatSync(directory).isDirectory(), "extension must be a local directory, not a symlink");
    const names = readdirSync(directory).sort();
    assert(names.includes("manifest.json"), "missing manifest.json");
    for (const name of names) assert(lstatSync(join(directory, name)).isFile(), "extension files must be regular, not links/directories");
    const manifest = JSON.parse(regularFile(join(directory, "manifest.json")));
    onlyKeys(manifest, ["manifest_version", "name", "version", "description", "minimum_chrome_version", "content_scripts"], "manifest");
    assert.equal(manifest.manifest_version, 3, "Manifest V3 required");
    assert.equal(typeof manifest.name, "string");
    assert(manifest.name.length > 0 && manifest.name.length <= 75, "invalid extension name");
    assert(typeof manifest.version === "string" && /^\d{1,5}(?:\.\d{1,5}){0,3}$/u.test(manifest.version), "invalid extension version");
    const versionParts = manifest.version.split(".");
    assert(versionParts.every(part => Number(part) <= 65535 && (part === "0" || !part.startsWith("0"))) && versionParts.some(part => Number(part) > 0), "invalid extension version component");
    assert(typeof manifest.minimum_chrome_version === "string" && /^\d+(?:\.\d+){0,3}$/u.test(manifest.minimum_chrome_version) && Number(manifest.minimum_chrome_version.split(".")[0]) >= 111, "static MAIN world requires Chrome >=111");
    assert(Array.isArray(manifest.content_scripts) && manifest.content_scripts.length === 1, "exactly one declared content script required");
    const script = manifest.content_scripts[0];
    onlyKeys(script, ["matches", "js", "world", "run_at", "all_frames"], "content script");
    assert(Array.isArray(script.matches) && script.matches.length > 0 && script.matches.every(validMatch), "explicit valid match patterns required");
    assert.equal(script.world, "MAIN");
    assert.equal(script.run_at, "document_start");
    assert(script.all_frames === undefined || script.all_frames === false, "only top frame is supported");
    assert(Array.isArray(script.js) && script.js.length > 0 && new Set(script.js).size === script.js.length, "local JS payload required");
    for (const name of script.js) {
        assert(typeof name === "string" && /^[^/\\:\u0000-\u001f\u007f]+\.js$/u.test(name) && name !== "..", "payload must be a local JS filename");
        assert(names.includes(name), `missing payload: ${name}`);
        checkJavaScript([join(directory, name)], "script");
    }
    assert.deepEqual(names, ["manifest.json", ...script.js].sort(), "unreferenced or non-JS files are not permitted");
    const temporary = mkdtempSync(join(tmpdir(), "rxd-hook-manifest-"));
    try {
        const file = join(temporary, "manifest.ts");
        // Actual pinned MV3 declarations, plus the stricter no-unrequested-fields checks above.
        writeFileSync(file, `const manifest = ${JSON.stringify(manifest)} satisfies chrome.runtime.ManifestV3;\n`);
        checkTypeScript([file], "chrome");
    } finally {
        rmSync(temporary, { recursive: true, force: true });
    }
}

try {
    const [mode, ...inputs] = process.argv.slice(2);
    assert(inputs.length > 0, "usage: validate.mjs js|ts FILE... | extension DIRECTORY");
    if (mode === "js") checkJavaScript(inputs.map(path => resolve(path)));
    else if (mode === "ts") checkTypeScript(inputs.map(path => resolve(path)));
    else if (mode === "extension" && inputs.length === 1) checkExtension(resolve(inputs[0]));
    else throw new Error("usage: validate.mjs js|ts FILE... | extension DIRECTORY");
    process.stdout.write(`offline ${mode} validation passed\n`);
} catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
}
