use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Cursor, Read},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

use crate::{ClassFile, Limits};

pub struct InputClass {
    pub entry: Option<String>,
    pub source_id: String,
    pub class: ClassFile,
}

pub struct Input {
    pub source_id: String,
    pub file_bytes: usize,
    pub classes: Vec<InputClass>,
    /// Every archive entry with its sizes, in central-directory order.
    pub entries: Vec<InputEntry>,
}

/// One archive entry, in the same shape dex reports for an APK.
#[derive(Debug, Clone)]
pub struct InputEntry {
    pub name: String,
    pub compression: u16,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
}

impl Input {
    /// Freeze input bytes before ZIP parsing or class analysis. No class loading.
    pub fn read(path: &Path, limits: Limits, release: u16) -> Result<Self> {
        ensure!(release >= 8, "INVALID_QUERY: release must be at least 8");
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "INVALID_INPUT: expected a regular file");
        ensure!(
            metadata.len() <= limits.input_bytes as u64,
            "RESOURCE_LIMIT: input byte budget exceeded"
        );
        let mut bytes = Vec::new();
        file.take(limits.input_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= limits.input_bytes,
            "RESOURCE_LIMIT: input byte budget exceeded"
        );
        let source_id = rxd_core::identity::content_id(path, &bytes);
        let file_bytes = bytes.len();
        let mut classes = Vec::new();
        let mut entries = Vec::new();
        if bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) {
            entries.push(InputEntry {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                compression: 0,
                compressed_size: file_bytes,
                uncompressed_size: file_bytes,
            });
            ensure!(
                bytes.len() <= limits.class_bytes,
                "RESOURCE_LIMIT: class byte budget exceeded"
            );
            classes.push(InputClass {
                entry: None,
                source_id: source_id.clone(),
                class: ClassFile::parse_bounded(bytes, limits.class_bytes)?,
            });
        } else {
            check_directory(&bytes, limits.entries)?;
            let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
                .context("INVALID_INPUT: invalid JAR/ZIP")?;
            ensure!(
                zip.len() <= limits.entries,
                "RESOURCE_LIMIT: archive entry budget exceeded"
            );
            for index in 0..zip.len() {
                let entry = zip.by_index(index)?;
                entries.push(InputEntry {
                    name: entry.name().to_owned(),
                    compression: match entry.compression() {
                        zip::CompressionMethod::Stored => 0,
                        zip::CompressionMethod::Deflated => 8,
                        _ => 255,
                    },
                    compressed_size: usize::try_from(entry.compressed_size()).unwrap_or(usize::MAX),
                    uncompressed_size: usize::try_from(entry.size()).unwrap_or(usize::MAX),
                });
            }
            let mut manifest = String::new();
            match zip.by_name("META-INF/MANIFEST.MF") {
                Ok(entry) => {
                    ensure!(entry.size() <= 1024 * 1024, "RESOURCE_LIMIT: manifest size");
                    entry.take(1024 * 1024 + 1).read_to_string(&mut manifest)?;
                }
                Err(zip::result::ZipError::FileNotFound) => {}
                Err(error) => return Err(error).context("INVALID_INPUT: unreadable JAR manifest"),
            }
            let multi_release = is_multi_release(&manifest)?;
            let mut selected = BTreeMap::<String, (u16, usize, String)>::new();
            let mut manifests = 0usize;
            for index in 0..zip.len() {
                let entry = zip.by_index(index)?;
                let name = entry.name();
                if name == "META-INF/MANIFEST.MF" {
                    manifests += 1;
                    ensure!(manifests == 1, "INVALID_INPUT: duplicate JAR manifest");
                }
                if !name.ends_with(".class") {
                    continue;
                }
                let (version, logical) = if let Some(rest) = name.strip_prefix("META-INF/versions/")
                {
                    if !multi_release {
                        continue;
                    }
                    let (number, logical) = rest
                        .split_once('/')
                        .context("INVALID_INPUT: multi-release entry")?;
                    let version: u16 = number
                        .parse()
                        .context("INVALID_INPUT: multi-release version")?;
                    ensure!(version >= 9, "INVALID_INPUT: multi-release version below 9");
                    if version > release {
                        continue;
                    }
                    (version, logical)
                } else {
                    (0, name)
                };
                if logical.starts_with("META-INF/") {
                    continue;
                }
                if let Some((old, _, _)) = selected.get(logical) {
                    ensure!(
                        *old != version,
                        "INVALID_INPUT: duplicate class entry {name}"
                    );
                    if *old > version {
                        continue;
                    }
                }
                selected.insert(logical.to_owned(), (version, index, name.to_owned()));
            }
            ensure!(
                !selected.is_empty(),
                "INVALID_INPUT: archive contains no selected JVM class files"
            );
            let mut expanded = 0usize;
            let mut metadata_bytes = 0usize;
            let mut declared_names = BTreeMap::new();
            for (_, (_, index, name)) in selected {
                let entry = zip.by_index(index)?;
                let size =
                    usize::try_from(entry.size()).context("RESOURCE_LIMIT: class size overflow")?;
                ensure!(
                    size <= limits.class_bytes,
                    "RESOURCE_LIMIT: class byte budget exceeded: {name}"
                );
                expanded = expanded
                    .checked_add(size)
                    .context("RESOURCE_LIMIT: expanded size overflow")?;
                ensure!(
                    expanded <= limits.expanded_bytes,
                    "RESOURCE_LIMIT: expanded byte budget exceeded"
                );
                let mut data = Vec::new();
                entry.take(size as u64 + 1).read_to_end(&mut data)?;
                ensure!(
                    data.len() == size,
                    "INVALID_INPUT: inconsistent class size: {name}"
                );
                let id = hash(&data);
                let class = ClassFile::parse_bounded(data, limits.class_bytes)
                    .with_context(|| name.clone())?;
                metadata_bytes = metadata_bytes
                    .checked_add(class.metadata_bytes)
                    .context("RESOURCE_LIMIT: metadata allocation overflow")?;
                ensure!(
                    metadata_bytes <= limits.expanded_bytes,
                    "RESOURCE_LIMIT: archive metadata budget exceeded"
                );
                ensure!(
                    declared_names
                        .insert(class.name.clone(), name.clone())
                        .is_none(),
                    "INVALID_INPUT: duplicate class definition {}",
                    class.name
                );
                classes.push(InputClass {
                    entry: Some(name),
                    source_id: id,
                    class,
                });
            }
        }
        Ok(Self {
            source_id,
            file_bytes,
            classes,
            entries,
        })
    }

    /// A method's coordinate: `Lclass;->name(proto)`, the same member identity DEX
    /// prints. It names the method exactly (the prototype separates overloads) and can
    /// be passed straight back to `--at`, with no content hash to assemble.
    pub fn method_at(class: &InputClass, index: usize) -> String {
        let method = &class.class.methods[index];
        format!(
            "L{};->{}{}",
            class.class.name, method.name, method.descriptor
        )
    }

    pub fn method_by_at(&self, coordinate: &str) -> Result<(&InputClass, usize)> {
        let Some((class_name, rest)) = coordinate.split_once("->") else {
            bail!("INVALID_QUERY: {coordinate} is not 'Lclass;->name(proto)'");
        };
        let Some((name, proto)) = rest.split_once('(') else {
            bail!("INVALID_QUERY: {coordinate} has no method prototype");
        };
        let descriptor = format!("({proto}");
        let class_name = class_name
            .strip_prefix('L')
            .and_then(|name| name.strip_suffix(';'))
            .unwrap_or(class_name);
        for class in &self.classes {
            if class.class.name != class_name {
                continue;
            }
            for (index, method) in class.class.methods.iter().enumerate() {
                if method.name == name && method.descriptor == descriptor {
                    return Ok((class, index));
                }
            }
        }
        bail!("INVALID_QUERY: {coordinate} is not a method of this input")
    }

    /// Select exactly one class by its internal, dotted or descriptor name.
    pub fn class_by_name(&self, name: &str) -> Result<&InputClass> {
        let wanted = normalize_class_name(name);
        let mut hits = self
            .classes
            .iter()
            .filter(|class| class.class.name == wanted);
        let selected = hits
            .next()
            .with_context(|| format!("INVALID_QUERY: class {name} not found in this input"))?;
        // Duplicate definitions are already rejected at read time, so a second
        // hit would be a logic error rather than ambiguous input.
        ensure!(
            hits.next().is_none(),
            "INVALID_QUERY: class {name} is defined more than once"
        );
        Ok(selected)
    }
}

/// Normalize a user class name (internal, dotted, or `Lpkg/Type;` descriptor)
/// to the internal slashed form stored on `ClassFile::name`.
pub fn normalize_class_name(name: &str) -> String {
    name.strip_prefix('L')
        .and_then(|rest| rest.strip_suffix(';'))
        .unwrap_or(name)
        .replace('.', "/")
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_multi_release(manifest: &str) -> Result<bool> {
    let mut headers: Vec<String> = Vec::new();
    for line in manifest.lines() {
        if line.is_empty() {
            break;
        }
        if let Some(continuation) = line.strip_prefix(' ') {
            headers
                .last_mut()
                .context("INVALID_INPUT: manifest continuation without header")?
                .push_str(continuation);
        } else {
            headers.push(line.to_owned());
        }
    }
    let mut selected = None;
    for header in headers {
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("Multi-Release")
        {
            ensure!(
                selected.is_none(),
                "INVALID_INPUT: duplicate Multi-Release header"
            );
            selected = Some(value.trim().eq_ignore_ascii_case("true"));
        }
    }
    Ok(selected.unwrap_or(false))
}

fn check_directory(bytes: &[u8], limit: usize) -> Result<()> {
    let offset = bytes.len().saturating_sub(65557);
    let tail = &bytes[offset..];
    let relative = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .context("INVALID_INPUT: expected class file or JAR/ZIP")?;
    let header = tail
        .get(relative..relative + 22)
        .context("INVALID_INPUT: truncated ZIP directory")?;
    let u16_at = |i| u16::from_le_bytes([header[i], header[i + 1]]);
    ensure!(
        u16_at(4) == 0 && u16_at(6) == 0 && u16_at(8) == u16_at(10),
        "UNSUPPORTED_INPUT: split ZIP archive"
    );
    ensure!(u16_at(10) != u16::MAX, "UNSUPPORTED_INPUT: ZIP64 archive");
    let u32_at = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
    let absolute = offset + relative;
    ensure!(
        u32_at(12) != u32::MAX
            && u32_at(16) != u32::MAX
            && absolute
                .checked_sub(20)
                .is_none_or(|at| bytes.get(at..at + 4) != Some(b"PK\x06\x07")),
        "UNSUPPORTED_INPUT: ZIP64 archive"
    );
    ensure!(
        usize::from(u16_at(10)) <= limit,
        "RESOURCE_LIMIT: archive entry budget exceeded"
    );
    ensure!(
        relative + 22 + usize::from(u16_at(20)) == tail.len(),
        "INVALID_INPUT: inconsistent ZIP trailer"
    );
    Ok(())
}
