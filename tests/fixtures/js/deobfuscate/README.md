# Deobfuscation fixtures

Files here are copied unchanged from `j4k0xb/webcrack` commit
`c80eec5f00622b86cea871d68349750ce950201f`, path
`packages/webcrack/src/deobfuscate/test/samples/`. `rotator-unary.js` is the
upstream `obfuscator.io-rotator-unary.js`; the other names are unchanged.

They are synthetic obfuscator.io outputs used by webcrack's own snapshot tests,
not live site code. Only the string-table and decoder portion is exercised here;
control-flow and self-defending expectations are not ported yet.

webcrack is MIT licensed; see `LICENSE-webcrack.txt`. The port keeps its own
detection order and conservative removal rules.
