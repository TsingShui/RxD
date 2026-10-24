# ADR 0009: Pin the ILSpy NativeAOT engine to v11.1

Status: accepted.

## Decision

Pin official `ICSharpCode.Decompiler` **11.1.0.9782**:

- Source: `a6909b2e1062d2404a52711926fa74030bd96119`.
- NuGet SHA-256: `1f76df4d35193ba4eeb50ffcd84eae381a2c48c67f95c1104dc406759448a811`.
- Verify the NuSpec repository commit and MIT license against the downloaded
  package.
- Keep `net10.0`, the C ABI, C# 10 output settings, fixed input bytes, framework
  reference restrictions, original IL/EH, mapping validation, budgets and atomic
  failures unchanged. No ILSpy source fork or runtime source modification.
- Implement the new `IAssemblyResolver.BeginSnapshot()` by delegating to the
  underlying resolver. This scopes its filesystem lookup cache; all actual
  resolution still traverses rxd's existing reference-name/key/path restrictions.
- Update build/package verification, benchmark identity, licenses and native
  upstream registry together.

## macOS comparison

Apple M3 Pro, 36 GiB, macOS 26.5.1/arm64; SDK 10.0.401, NativeAOT 10.0.12.
Both libraries used the same settings and 167 .NET 10.0.12 reference DLLs. The
same release CLI binary selected each dylib at runtime. Preparation warmed OS
file caches; no cache flushing, CPU isolation, forced GC or frequency pinning.
Each workload/version had 20 fresh CLI processes and 20 fresh C ABI processes;
40 repeat ABI calls were collected across two additional processes. Version
order alternated AB/BA, workload order rotated, and repeat batch order reversed.
All **640 timed calls** succeeded; selected C# was identical across versions.

Fresh CLI wall time includes startup, analysis worker, input read/hash,
validation, native load/initialization/reconstruction, serialization and stdout.
Values below are median/P95 in milliseconds, separately per workload:

| Selected method | Input bytes / IL bytes | v10 CLI | v11 CLI | >150 ms, each version |
|---|---:|---:|---:|---:|
| Synthetic Fixture.Add | 4,096 / 4 | 22.06/23.53 | 22.47/24.18 | 0/20 |
| CoreLib Math.Abs(int32) | 15,767,040 / 19 | 111.38/115.65 | 110.32/118.21 | 0/20 |
| System.Linq Enumerable.Count | 794,448 / 107 | 27.24/28.24 | 27.22/28.06 | 0/20 |
| System.Text.Json JsonElement.GetString | 2,101,544 / 25 | 31.13/32.40 | 31.66/32.63 | 0/20 |

CoreLib is the hash-pinned corpus
`a6acd5d87523255556bc17d964779aae71da1809623710aa4c6d091061b3246a`,
MethodDef `0x06000544`. Its fresh-process ABI-call medians were 6.77/6.84 ms
(v10/v11); repeat medians were 4.56/4.52 ms. ABI timings exclude file read and
library initialization and cannot substitute for CLI acceptance.

These four selected methods are not a whole-corpus or semantic-equivalence
proof. Linux/Windows correctness, native packaging and latency still require
their separate release gates.

## Android comparison

A non-debuggable Android App compared the same four targets on ARM64 with
ordinary App sandboxing. All 480 timed calls succeeded with identical selected
C# and mappings across versions. This corroborates the engine choice but is not
Android product support, a full CLI latency result, or a 16 KiB-runtime result.

## Amendment: reconstruction warnings are returned, not refused

The original integration aborted every body for which ILSpy recorded a warning
(`il_warning`) and returned only original IL. That discarded usable output: in a
63-instruction, handler-free method whose every operand resolved, the engine still
refused, and the ABI carried no warning text, so neither the caller nor `rxd` could say
what the engine objected to.

The engine now returns the reconstruction together with its diagnostics:

- `Payload.warnings` carries the deduplicated `ILFunction.Warnings` text, prefixed with
  the owning method name, so a class-scope failure names the member;
- `detail.warnings` exposes that array next to `code`; `decompiled` stays `true`;
- an `InvalidExpression`/`InvalidBranch` node is a diagnostic as well
  (`unreconstructed expression at IL_xxxx`), never an abort: the surrounding
  reconstruction is still returned;
- a source-mapping entry the selected body cannot prove is dropped and counted in
  `detail.warnings` (`mapping: dropped N unprovable source entry/entries`) instead of
  discarding the reconstruction.

Only an engine that returns no payload at all — crash, exception, absent entry point or a
reached resource budget — degrades the record to original IL.

This changes observable output for bodies that previously degraded to `il_warning`.
Desktop engines are now produced per target by `scripts/build-ilspy.py` during the
release build, so the source change is compiled into each distributed engine. The
separate Android App bundle remains a checked-in artifact and requires its own
producer/import when shared native sources change;
`scripts/prebuilt-ilspy.py --android-app check` compares its manifest's
`source_inputs` with the current source and fails until they match.
