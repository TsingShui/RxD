# Native hook generation fixtures

All C/byte fixtures here are independently authored for #135 under Apache-2.0.
Never execute these binaries or generated hooks. They contain no external library
code. `fixtures.rs` writes ELF headers/symbols directly; other fixtures were built
with Homebrew LLVM/LLD 23.1.1 on macOS arm64. Normal tests use committed bytes,
not a compiler, a runtime target, or generator-derived address expectations.

## Independent expectations

- Handwritten ELF: mapped file header at VMA `0x700000`; `.text` file offset
  `0x1000`, VMA `0x702000`; `observe` entry `0x702000`; module offset `0x2000`.
  Additional tests independently change this layout to diagnose ambiguity,
  stripping, unsupported architecture, and precision above JS safe integers.
- `typed.elf`: ELF64 x86-64, Linux SysV; header PT_LOAD `0x200000`, `.text`
  PT_LOAD `0x402000` / file `0x1000`. `llvm-nm -n` gives signed_scalar
  `0x402000`, wide_scalar `0x402010`, signed_wide `0x402020`, pointer_scalar
  `0x402028`, mixed_scalar `0x402030`, unsupported_float `0x40203d`,
  unsupported_variadic `0x402040`, unrelated `0x402050`, caller `0x402090`.
  Original C declarations and embedded DWARF give independent parameter/type
  expectations. `unrelated` is deliberately larger than the selected scalar.
- `inferred.elf`: same mapped code/symbols, no DWARF. `stripped.elf`: same code,
  no symbols or DWARF; retained `.eh_frame` supplies independent entry extents.
  `0x402001` is inside signed_scalar, never a declared entry.
- `thumb.so`: ELF32 ARMv7 Thumb; `observe` symbol `0x2001`, actual code
  `0x2000`, runtime module offset `0x2000` with Thumb bit set for attach.
- `arm64.so`: ELF64 AArch64; `observe` `0x2000`, module base `0`.
- `mapping.dll`: PE x64 DLL; preferred image base `0x180000000`, executable
  export `observe` RVA `0x1000`, static VMA `0x180001000`.
- `mapping.dylib`: thin Mach-O arm64 dylib; `__TEXT` base `0`, `_observe`
  `0x2a8`. No universal-slice or arm64e interpretation is implied.
- `mapping-executable`: stripped thin Mach-O arm64 executable, header base
  `0x100000000`, LC_MAIN **file offset** `0x2f0`, static entry VMA
  `0x1000002f0`. Its remaining `__mh_execute_header` symbol is not a function.
  The unmapped 4-GiB __PAGEZERO reservation is not executable discovery work.

Expectations were inspected with `llvm-readobj --file-headers --program-headers
--symbols --coff-exports`, `llvm-nm -n`, and `llvm-dwarfdump --name=signed_scalar`.
- The PE Thumb mode test additionally reuses pinned upstream
  `third_party/kuna/decompiler/crates/kuna-analysis/tests/fixtures/armv4t_thumb_pe.exe`
  and its independent `armv4t_thumb_pe.s` generator (Apache-2.0; SHA-256
  `49c9bd89ae54ad720a854d91adace9a2822688eef380179175574fb1eb676a1e`).
  THUMB/ARMNT machine variants with an even entry RVA must still attach in Thumb
  mode. Its debug typed semantic test explicitly allows 10s analysis because ARM
  spec loading is slow without optimization; release latency is measured separately.

They are not produced by rxd. These tiny fixtures establish correctness, not
representative production latency or runtime hookability.

## Rebuild (LLVM 23.1.1)

From repository root, with LLVM clang and LLD on PATH:

```sh
D=crates/rxd-hook-gen/tests/native
clang --target=x86_64-linux-gnu -g -gdwarf-4 -ffile-prefix-map="$PWD"=. -fdebug-compilation-dir=. -O1 -fno-inline -fno-pie -nostdlib -fuse-ld=lld -Wl,-Ttext=0x402000,-e,caller,--build-id=none -no-pie "$D/typed.c" -o "$D/typed.elf"
llvm-objcopy --strip-debug "$D/typed.elf" "$D/inferred.elf"
llvm-objcopy --strip-all "$D/typed.elf" "$D/stripped.elf"
clang --target=armv7-linux-gnueabihf -mthumb -O1 -fuse-ld=lld -nostdlib -shared -Wl,-Ttext=0x2000,--build-id=none "$D/mapping.c" -o "$D/thumb.so"
clang --target=aarch64-linux-gnu -O1 -fuse-ld=lld -nostdlib -shared -Wl,-Ttext=0x2000,--build-id=none "$D/mapping.c" -o "$D/arm64.so"
clang --target=x86_64-pc-windows-msvc -O1 -c "$D/mapping.c" -o /tmp/mapping.obj
lld-link /dll /noentry /nodefaultlib /export:observe /base:0x180000000 /out:"$D/mapping.dll" /tmp/mapping.obj
# Remove mapping.lib (an unused linker intermediate).
clang --target=arm64-apple-macos11 -O1 -c "$D/mapping.c" -o /tmp/mapping.o
ld64.lld -dylib -arch arm64 -platform_version macos 11.0 11.0 -install_name /usr/local/lib/mapping.dylib -o "$D/mapping.dylib" /tmp/mapping.o
ld64.lld -arch arm64 -platform_version macos 11.0 11.0 -e _observe -o "$D/mapping-executable" /tmp/mapping.o
llvm-strip --strip-all "$D/mapping-executable"
```

SHA-256:

```
2b81116e2a05aa270147a7a99b08ee4b9e92fdc009e4f8f99f4e4851f9395b36  typed.elf
353d791b53c101bc0207dfbaf2bf4269a575e53dabf40985a8b9e754e901f53c  inferred.elf
b9c4b711b596639c68d736fb7813b06fa13039d13c6d8d11a55a0900528f8591  stripped.elf
0a7ec5aa4bab317d379092a2510c7c447655258dc11c99523b8257385feb536d  thumb.so
2069a347ab2a6902e89abf93cac9c859d75b60297df19f8488c40d2029e1fef8  arm64.so
f2fdf4181223ecfb49e246dbe8c66ea3bd41bce2d8ca7e62ea460ab22c1ec879  mapping.dll
0e0bb691b2e72ab871e10c67614a021bd5fb18c5938e738f7829e3982be16fb2  mapping.dylib
e57548a13b5296c16ee0c58b38d9274a9d773f1b29c707632380fbb85f33c3a9  mapping-executable
```

Engine E2E requires `--features native,kuna` and explicit prepared resource path
`RXD_HOOK_TEST_SPECS=.../specs-build` (the root containing `Ghidra/`). No engine
cases are silently skipped when that feature is enabled. Generation never
installs tooling or mutates process-global engine environment. Pinned offline
Node/TS tooling is documented in `devtools/hook-gen/README.md`.
