# String navigation fixture

`Strings.dll` is an original, synthetic ECMA-335 PE assembled by the Python
stdlib-only `generate.py`. It has no entry point and is never executed or loaded
as a CLR assembly. Regenerate with `python3 tests/fixtures/dotnet/strings/generate.py`.
`expected.json` records the independently authored heap entries, MethodDef owners,
load offsets, original IL and file positions; none are inferred from rxd output.

Two classes and three methods cover repeated loads, multiple owners, separate
equal-value heap entries, unused inventory, empty strings, supplementary Unicode,
embedded NUL, lone high/low surrogates and a readable substring beside a surrogate.
The fixture and generator are original project test artifacts under the repository
license. SHA-256 and size are recorded in the expectation file.
