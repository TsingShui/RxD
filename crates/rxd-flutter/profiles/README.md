# Platform profiles

`elf-arm64.json` is `profiles/platform/elf-arm64.json` from `dae-rs` 0.1.14
(https://github.com/ejfkdev/dae, MIT) — the ELF/ARM64 container description rxd
needs because the upstream crate embeds it privately in its CLI. It is data only:
class-id layouts, tagging and container offsets. Keep it byte-identical to the
pinned crate version so a profile and its analyzer always agree.
