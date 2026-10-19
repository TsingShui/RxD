# Original aggregate .NET fixture

`Closure.dll` is repository-owned C# fixture source, compiled only for static
metadata and IL analysis. Tests never load or execute its methods or referenced
native library. It covers a generic class with constraints, explicit interface,
base/sibling exclusion, property/event accessors, constructors, depth-two nested
types, abstract/PInvoke declarations, struct and enum declarations, and catch EH.

Ordinary CLI E2E reads the committed DLL and `expected.json` offline without an
SDK. Explicit regeneration is `python3 tests/fixtures/dotnet/closure/generate.py`
with the pinned SDK 10.0.401 and trusted PEReader inspector. The oracle reads
original metadata, IL and ordered EH through System.Reflection.Metadata and
System.Reflection.PortableExecutable; it never derives expectations from rxd or
loads the target assembly. No NuGet packages are referenced. Exact SDK/compiler,
source and input SHA-256, framework version, MVID and timestamp are recorded in
`expected.json`. Build/inspector outputs remain under ignored `target/`.

Independent table-file offsets permit focused corruption tests for cyclic nested
ownership, foreign accessor relationships, invalid RVA and method headers.
