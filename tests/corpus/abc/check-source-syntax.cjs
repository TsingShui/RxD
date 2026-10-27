// Parse reconstructed source only; never link or execute analyzed modules.
const fs = require("node:fs");
const readline = require("node:readline");
const [parserPath, input, output] = process.argv.slice(2);
if (!parserPath || !input || !output) throw new Error("usage: node check-source-syntax.cjs PARSER SOURCE_JSONL REPORT");
const parser = require(parserPath);
const failures = [];
let count = 0;
(async () => {
    const lines = readline.createInterface({input: fs.createReadStream(input), crlfDelay: Infinity});
    for await (const line of lines) {
        const item = JSON.parse(line);
        count++;
        try { parser.parse(item.source, {sourceType: "module"}); }
        catch (error) { failures.push({record: item.record, method: item.method, error: String(error)}); }
    }
    fs.writeFileSync(output, JSON.stringify({parsed: count, failures}, null, 2) + "\n", {flag: "wx"});
    console.log(JSON.stringify({parsed: count, failed: failures.length, first: failures.slice(0, 10)}, null, 2));
    if (failures.length) process.exitCode = 1;
})().catch(error => { console.error(error); process.exitCode = 1; });
