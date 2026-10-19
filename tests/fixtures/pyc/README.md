# PYC decompilation fixtures

These two offline bytecode files come from upstream pycdc test fixtures at commit
`25ed1ab2`, covered by the imported MIT license at `third_party/pycdc/LICENSE`. Tests
never execute the bytecode or require a Python interpreter.

The additional E2E boundary inputs in `tests/pyc/boundaries.rs` are handwritten CPython
3.12 marshal layouts (unknown opcode, Unicode literal, depth/reference expansion and long
conversion). They are synthetic regression diagnostics, not production performance
evidence; no compiler or interpreter executes them. Content covers exact-root/trailing
data, operand/prefix/cache truncation, nested function/lambda/class/generic-wrapper/
comprehension and adjacent valid boundary cases. Modern opcode values and cache widths
are checked against CPython 3.12's documented encoding and the pinned 3.12.0 opcode
table; legacy operand mutations replace only the declared `co_code` bytes/length in the
licensed 2.7 fixture. No expectation is generated from rxd output.

Encoding references:
- https://docs.python.org/3.12/library/dis.html
- https://github.com/python/cpython/blob/v3.12.0/Lib/opcode.py
- https://github.com/python/cpython/blob/v2.7.18/Lib/opcode.py

| File | Python | SHA-256 |
| --- | --- | --- |
| `hello.3.12.pyc` | 3.12 | `5bc59d39298f340771af7252c0623dc2c2ecc571d087f2b55ad2bcde2e53e288` |
| `simple_const.2.7.pyc` | 2.7 | `255853e2da6aa41ec0ad5130671b9b191f87792057629792267f187929b1a4fe` |
