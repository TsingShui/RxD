//! Dart AOT (Flutter) snapshot navigation and reconstruction through `dae-rs`.
//!
//! Listing commands read the snapshot's library and function tables only; source
//! reconstruction runs the upstream decompiler as a separate, explicit step.
use std::path::{Path, PathBuf};

use dae::analyzer::Analyzer;
use dae::locale;
use dae::platform::locate_snapshots;
use dae::profile::detect::detect_or_default;
use dae::profile::{PlatformProfile, SdkProfile, parse_platform};
use rxd_core::{Error, Result};

/// One admitted libapp snapshot with its detected platform and SDK profile.
pub struct Snapshot {
    #[allow(dead_code)]
    path: PathBuf,
    bytes: Vec<u8>,
    platform: PlatformProfile,
    sdk: &'static SdkProfile,
    fallback: bool,
}

impl Snapshot {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|error| Error::new("IO_ERROR", format!("{}: {error}", path.display())))?;
        // Android Flutter ships ELF/ARM64; other containers are not admitted.
        if !bytes.starts_with(b"\x7fELF") || bytes.len() < 20 || bytes[18] != 0xB7 {
            return Err(Error::new(
                "UNSUPPORTED_PLATFORM",
                "expected an ELF ARM64 libapp.so snapshot",
            ));
        }
        let platform = parse_platform(include_str!("../profiles/elf-arm64.json"))
            .map_err(|error| Error::new("INVALID_INPUT", error))?;
        let (offsets, fallback) = locate_snapshots(&bytes, &platform)
            .map_err(|error| Error::new("INVALID_INPUT", error))?;
        let messages = locale::messages(locale::detect());
        let sdk = detect_or_default(&bytes, offsets, &messages);
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            platform,
            sdk,
            fallback,
        })
    }

    pub fn input_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// Snapshot-level facts: detected SDK profile, coverage and entry counts.
    pub fn info(&self) -> Result<Info> {
        let analyzer = self.analyzer()?;
        let groups = analyzer.build_functions(false);
        let libraries = groups.len();
        let functions = groups
            .iter()
            .map(|(_, classes)| {
                classes
                    .iter()
                    .map(|(_, entries)| entries.len())
                    .sum::<usize>()
            })
            .sum();
        Ok(Info {
            sdk: self.sdk().abi.clone(),
            status: self.sdk().status.clone(),
            compressed_pointers: self.sdk().compressed_pointers,
            libraries,
            functions,
            platform_warning: self.platform_warning(),
        })
    }

    /// Append one TAB-separated row per function, in library then class order.
    ///
    /// Rows are written straight into `out`: the snapshot already holds the
    /// names, so materializing a row struct per function only adds allocations.
    pub fn functions_into(
        &self,
        out: &mut String,
        filter: Option<&rxd_core::filter::Contains>,
    ) -> Result<usize> {
        let analyzer = self.analyzer()?;
        let groups = analyzer.build_functions(false);
        let mut written = 0;
        for (lib, classes) in &groups {
            for (class, entries) in classes {
                for entry in entries {
                    if let Some(filter) = filter
                        && !filter.matches(lib)
                        && !filter.matches(class)
                        && !filter.matches(&entry.mangled)
                    {
                        continue;
                    }
                    out.push_str("0x");
                    push_hex(out, entry.ep);
                    out.push('\t');
                    push_atom(out, lib);
                    out.push_str("::");
                    push_atom(out, class);
                    out.push_str("::");
                    push_atom(out, &entry.mangled);
                    out.push('\n');
                    written += 1;
                }
            }
        }
        Ok(written)
    }

    /// Reconstruct Dart-like pseudocode for the selected libraries.
    pub fn decompile(&self, only: Option<&str>) -> Result<Vec<Rendered>> {
        let analyzer = self.analyzer()?;
        let mut groups = analyzer.build_functions(false);
        if let Some(prefix) = only {
            groups.retain(|(lib, _)| lib.starts_with(prefix));
            if groups.is_empty() {
                return Err(Error::new(
                    "INVALID_QUERY",
                    format!("no library matches {prefix:?}"),
                ));
            }
        }
        let (files, _) = dae::decompiler::render(&analyzer, &groups)
            .map_err(|error| Error::new("DECOMPILE_FAILED", error))?;
        Ok(files
            .into_iter()
            .map(|(name, source)| Rendered { name, source })
            .collect())
    }

    fn sdk(&self) -> &'static SdkProfile {
        self.sdk
    }

    fn platform_warning(&self) -> Option<String> {
        self.fallback
            .then(|| "platform symbols missing; snapshot located by magic fallback".to_owned())
    }

    fn analyzer(&self) -> Result<Analyzer<'_>> {
        Analyzer::new_without_names(&self.bytes, self.sdk, &self.platform)
            .map_err(|error| Error::new("INVALID_INPUT", error))
    }
}

/// Snapshot facts reported by `rxd dae info`.
pub struct Info {
    pub sdk: String,
    pub status: String,
    pub compressed_pointers: bool,
    pub libraries: usize,
    pub functions: usize,
    pub platform_warning: Option<String>,
}

fn push_hex(out: &mut String, value: u64) {
    let mut buffer = [0u8; 16];
    let mut index = buffer.len();
    let mut rest = value;
    loop {
        index -= 1;
        buffer[index] = b"0123456789abcdef"[(rest & 0xF) as usize];
        rest >>= 4;
        if rest == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&buffer[index..]).expect("hex is ASCII"));
}

/// Names come from the snapshot; quote them only when a cell would break the row.
fn push_atom(out: &mut String, value: &str) {
    if value
        .bytes()
        .any(|byte| byte == b'\t' || byte == b'\n' || byte == b'"' || byte == b'\\')
    {
        out.push_str(&serde_json::to_string(value).unwrap_or_default());
    } else {
        out.push_str(value);
    }
}

/// One reconstructed library file.
pub struct Rendered {
    pub name: String,
    pub source: String,
}
