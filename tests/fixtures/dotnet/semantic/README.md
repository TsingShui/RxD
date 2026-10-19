# Original metadata-reference proof fixture

`References.dll` is an independently assembled PE/ECMA-335 analysis fixture. It
contains no executable entry point and is never loaded or executed. All source
and generated fixture data are original project data under the repository license.

Regenerate offline with Python 3 standard library:

```sh
python3 tests/fixtures/dotnet/semantic/generate.py
```

`expected.json` records the assembly SHA-256, every independently specified
method's original IL and RVA, and each explicit method operand site plus field/type caller sites assembled by the
generator. It is not generated from rxd output. The fixed metadata layout covers
Module-scope TypeRef aliases, external AssemblyRef versions, a same-named ModuleRef,
nested type ownership, constructor/handle/ldtoken/jmp sites, overloads, ambiguous
local definitions, constructed generic owners and distinct MethodSpecs. Scoped
named types and embedded TypeSpecs prove that full structural signature identity
preserves type scope and concrete arguments. The 1,002-entry user-string heap
also tests complete validation beyond upstream's first-1,000-entry content pass.

This synthetic boundary fixture does not establish production latency. Published
production and runtime metadata/operand oracles are pinned separately under
`tests/corpus/dotnet/` and used only by the release benchmark.

The field/type extension preserves the original eleven method indices and all
original caller sites. A twelfth caller independently assembles field loads,
stores, addresses and handles; same-name fields with different signatures,
ambiguous definitions, nested and external owners; Holder<int>/Holder<string>
instances; explicit direct/alias/constructed type handles, castclass and
constrained. followed by callvirt; and array/pointer/generic-variable TypeSpecs.
`field_type_sites` is the exact assembler oracle for this added caller. Type hits
must exclude field/method owners and structural inner types of arrays/pointers.

`--expanded-signature --output tests/fixtures/dotnet/semantic/expanded` regenerates
an original resource-boundary variant (8,192 bytes, SHA-256
`fc16b016bb60f334642ef1d827a1cc2045d0b822ce7fb112f3cf51aded202b16`).
Holder's owner argument is a function pointer with 1,000 int32 parameters;
Echo repeats !0 in its return and 100 parameters. It tests that generic
substitution charges the entire expanded signature before cloning and checks
encoded descriptors before JSON materialization. This artificial input is not a
production latency sample and is never executed.
