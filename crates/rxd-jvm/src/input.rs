use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Cursor, Read},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use rayon::prelude::*;

#[derive(Default)]
struct ArchiveBudget {
    entries: usize,
    expanded: usize,
    metadata: usize,
}

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

#[derive(Debug, Clone)]
pub struct InputEntry {
    pub name: String,
    pub compression: u16,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
}

impl Input {
    /// Read only the archive directory: `entries` never parses class files.
    pub fn read_entries(
        path: &Path,
        limits: Limits,
        release: u16,
    ) -> Result<(String, usize, Vec<InputEntry>)> {
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
        let source_id = rxd_core::identity::input_id(&bytes);
        let file_bytes = bytes.len();
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
            let mut budget = ArchiveBudget {
                entries: zip.len(),
                ..ArchiveBudget::default()
            };
            let nested = nested_entries(&mut zip, limits, 0, &mut budget)?;
            entries.extend(nested);
        }
        Ok((source_id, file_bytes, entries))
    }

    /// Parse one coordinate-selected class; None requests full-reader fallback.
    pub fn read_class_scoped(
        path: &Path,
        limits: Limits,
        release: u16,
        coordinate: &str,
    ) -> Result<Option<Self>> {
        ensure!(release >= 8, "INVALID_QUERY: release must be at least 8");
        if coordinate.contains("->") || coordinate.is_empty() {
            return Ok(None);
        }
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
        Self::read_scoped_bytes(
            bytes,
            limits,
            release,
            coordinate,
            0,
            &mut ArchiveBudget::default(),
        )
    }

    fn read_scoped_bytes(
        mut bytes: Vec<u8>,
        limits: Limits,
        release: u16,
        coordinate: &str,
        depth: usize,
        budget: &mut ArchiveBudget,
    ) -> Result<Option<Self>> {
        ensure!(depth <= 16, "RESOURCE_LIMIT: nested archive depth exceeded");
        let source_id = rxd_core::identity::input_id(&bytes);
        let file_bytes = bytes.len();
        let (archive, coordinate) = coordinate
            .rsplit_once('!')
            .map_or((None, coordinate), |(archive, local)| {
                (Some(archive), local)
            });
        if let Some(archive) = archive {
            for (index, name) in archive.split('!').enumerate() {
                ensure!(
                    depth + index < 16,
                    "RESOURCE_LIMIT: nested archive depth exceeded"
                );
                check_directory(&bytes, limits.entries)?;
                let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
                budget.entries = budget
                    .entries
                    .checked_add(zip.len())
                    .context("RESOURCE_LIMIT: entry count overflow")?;
                ensure!(
                    budget.entries <= limits.entries,
                    "RESOURCE_LIMIT: archive entry budget exceeded"
                );
                let mut found = None;
                for index in 0..zip.len() {
                    if zip.by_index(index)?.name() == name {
                        ensure!(
                            found.replace(index).is_none(),
                            "INVALID_INPUT: duplicate nested archive entry"
                        );
                    }
                }
                let index = found.context("NOT_FOUND: nested archive does not exist")?;
                let entry = zip.by_index(index)?;
                let size = usize::try_from(entry.size())?;
                budget.expanded = budget
                    .expanded
                    .checked_add(size)
                    .context("RESOURCE_LIMIT: expanded size overflow")?;
                ensure!(
                    budget.expanded <= limits.expanded_bytes,
                    "RESOURCE_LIMIT: expanded byte budget exceeded"
                );
                bytes = Vec::new();
                entry.take(size as u64 + 1).read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() == size,
                    "INVALID_INPUT: inconsistent nested archive size"
                );
            }
        }
        if bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) {
            return Ok(None);
        }
        check_directory(&bytes, limits.entries)?;
        let mut zip =
            zip::ZipArchive::new(Cursor::new(bytes)).context("INVALID_INPUT: invalid JAR/ZIP")?;
        ensure!(
            zip.len() <= limits.entries,
            "RESOURCE_LIMIT: archive entry budget exceeded"
        );
        let mut entries = Vec::new();
        let mut manifest = String::new();
        match zip.by_name("META-INF/MANIFEST.MF") {
            Ok(entry) => {
                ensure!(entry.size() <= 1024 * 1024, "RESOURCE_LIMIT: manifest size");
                entry.take(1024 * 1024 + 1).read_to_string(&mut manifest)?;
            }
            Err(zip::result::ZipError::FileNotFound) => {}
            Err(error) => return Err(error.into()),
        }
        let multi_release = is_multi_release(&manifest)?;
        let mut candidate: Option<(u16, usize)> = None;
        let internal = normalize_class_name(coordinate);
        let wanted = format!("{internal}.class");
        let mut nested = Vec::new();
        let mut entry_names = std::collections::BTreeSet::new();
        let mut manifests = 0;
        for index in 0..zip.len() {
            let entry = zip.by_index(index)?;
            let name = entry.name().to_owned();
            ensure!(
                entry_names.insert(name.clone()),
                "INVALID_INPUT: duplicate archive entry"
            );
            ensure!(
                !name.contains('!'),
                "INVALID_INPUT: ambiguous archive entry name"
            );
            if name == "META-INF/MANIFEST.MF" {
                manifests += 1;
                ensure!(manifests == 1, "INVALID_INPUT: duplicate JAR manifest");
            }
            if name.to_ascii_lowercase().ends_with(".jar")
                || name.to_ascii_lowercase().ends_with(".aar")
                || name.to_ascii_lowercase().ends_with(".zip")
            {
                nested.push((index, name.clone()));
            }
            entries.push(InputEntry {
                name: name.clone(),
                compression: match entry.compression() {
                    zip::CompressionMethod::Stored => 0,
                    zip::CompressionMethod::Deflated => 8,
                    _ => 255,
                },
                compressed_size: usize::try_from(entry.compressed_size()).unwrap_or(usize::MAX),
                uncompressed_size: usize::try_from(entry.size()).unwrap_or(usize::MAX),
            });
            let (version, logical) = if let Some(rest) = name.strip_prefix("META-INF/versions/") {
                if !multi_release || !name.ends_with(".class") {
                    continue;
                }
                let (version, logical) = rest
                    .split_once('/')
                    .context("INVALID_INPUT: multi-release entry")?;
                let version: u16 = version
                    .parse()
                    .context("INVALID_INPUT: multi-release version")?;
                ensure!(version >= 9, "INVALID_INPUT: multi-release version below 9");
                if version > release {
                    continue;
                }
                (version, logical)
            } else {
                (0, name.as_str())
            };
            if logical == wanted {
                if let Some((old, _)) = candidate {
                    ensure!(
                        old != version,
                        "INVALID_INPUT: duplicate class entry {name}"
                    );
                    if old > version {
                        continue;
                    }
                }
                candidate = Some((version, index));
            }
        }
        budget.entries = budget
            .entries
            .checked_add(zip.len())
            .context("RESOURCE_LIMIT: entry count overflow")?;
        ensure!(
            budget.entries <= limits.entries,
            "RESOURCE_LIMIT: archive entry budget exceeded"
        );
        let mut nested_classes = Vec::new();
        for (index, name) in nested {
            ensure!(
                !name.contains('!'),
                "INVALID_INPUT: ambiguous nested archive name"
            );
            let entry = zip.by_index(index)?;
            let size = usize::try_from(entry.size())?;
            budget.expanded = budget
                .expanded
                .checked_add(size)
                .context("RESOURCE_LIMIT: expanded size overflow")?;
            ensure!(
                budget.expanded <= limits.expanded_bytes,
                "RESOURCE_LIMIT: expanded byte budget exceeded"
            );
            let mut data = Vec::new();
            entry.take(size as u64 + 1).read_to_end(&mut data)?;
            ensure!(
                data.len() == size,
                "INVALID_INPUT: inconsistent nested archive size"
            );
            let Some(inner) = Self::read_scoped_bytes(
                data,
                limits,
                release,
                coordinate,
                depth + archive.map_or(0, |archive| archive.split('!').count()) + 1,
                budget,
            )?
            else {
                return Ok(None);
            };
            let prefix = archive.map_or(name.clone(), |archive| format!("{archive}!{name}"));
            for mut class in inner.classes {
                class.entry = Some(format!("{prefix}!{}", class.entry.unwrap_or_default()));
                nested_classes.push(class);
            }
        }
        let Some((_, index)) = candidate else {
            return Ok(Some(Self {
                source_id,
                file_bytes,
                classes: nested_classes,
                entries,
            }));
        };
        let entry = zip.by_index(index)?;
        let wanted = entry.name().to_owned();
        let size = usize::try_from(entry.size()).context("RESOURCE_LIMIT: class size overflow")?;
        ensure!(
            limits.class_bytes.is_none_or(|limit| size <= limit),
            "RESOURCE_LIMIT: class byte budget exceeded: {wanted}"
        );
        ensure!(
            size <= limits.expanded_bytes.saturating_sub(budget.expanded),
            "RESOURCE_LIMIT: expanded byte budget exceeded"
        );
        let mut data = Vec::new();
        entry.take(size as u64 + 1).read_to_end(&mut data)?;
        ensure!(
            data.len() == size,
            "INVALID_INPUT: inconsistent class size: {wanted}"
        );
        budget.expanded = budget
            .expanded
            .checked_add(size)
            .context("RESOURCE_LIMIT: expanded size overflow")?;
        let class_id = hash(&data);
        let class = ClassFile::parse_bounded(data, limits.expanded_bytes)
            .with_context(|| wanted.clone())?;
        budget.metadata = budget
            .metadata
            .checked_add(class.metadata_bytes)
            .context("RESOURCE_LIMIT: metadata allocation overflow")?;
        ensure!(
            budget.metadata <= limits.expanded_bytes,
            "RESOURCE_LIMIT: archive metadata budget exceeded"
        );
        if class.name != internal {
            // The entry path and the declared class name disagree; use the full reader.
            return Ok(None);
        }
        nested_classes.insert(
            0,
            InputClass {
                entry: Some(
                    archive.map_or(wanted.clone(), |archive| format!("{archive}!{wanted}")),
                ),
                source_id: class_id,
                class,
            },
        );
        Ok(Some(Self {
            source_id,
            file_bytes,
            classes: nested_classes,
            entries,
        }))
    }

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
        Self::read_bytes(
            bytes,
            path,
            limits,
            release,
            0,
            &mut ArchiveBudget::default(),
        )
    }

    fn read_bytes(
        bytes: Vec<u8>,
        path: &Path,
        limits: Limits,
        release: u16,
        depth: usize,
        budget: &mut ArchiveBudget,
    ) -> Result<Self> {
        ensure!(depth <= 16, "RESOURCE_LIMIT: nested archive depth exceeded");
        let source_id = rxd_core::identity::input_id(&bytes);
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
                limits.class_bytes.is_none_or(|limit| bytes.len() <= limit),
                "RESOURCE_LIMIT: class byte budget exceeded"
            );
            classes.push(InputClass {
                entry: None,
                source_id: source_id.clone(),
                class: ClassFile::parse_bounded(bytes, limits.expanded_bytes)?,
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
            budget.entries = budget
                .entries
                .checked_add(zip.len())
                .context("RESOURCE_LIMIT: archive entry count overflow")?;
            ensure!(
                budget.entries <= limits.entries,
                "RESOURCE_LIMIT: archive entry budget exceeded"
            );
            let mut nested = Vec::new();
            let mut names = std::collections::BTreeSet::new();
            for index in 0..zip.len() {
                let entry = zip.by_index(index)?;
                ensure!(
                    !entry.name().contains('!'),
                    "INVALID_INPUT: ambiguous archive entry name"
                );
                ensure!(
                    names.insert(entry.name().to_owned()),
                    "INVALID_INPUT: duplicate archive entry"
                );
                let lower = entry.name().to_ascii_lowercase();
                if lower.ends_with(".jar") || lower.ends_with(".aar") || lower.ends_with(".zip") {
                    nested.push((index, entry.name().to_owned()));
                }
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
            let mut expanded = budget.expanded;
            let mut pending = Vec::new();
            let mut metadata_bytes = 0usize;
            let mut declared_names = BTreeMap::new();
            for (_, (_, index, name)) in selected {
                let entry = zip.by_index(index)?;
                let size =
                    usize::try_from(entry.size()).context("RESOURCE_LIMIT: class size overflow")?;
                ensure!(
                    limits.class_bytes.is_none_or(|limit| size <= limit),
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
                pending.push((name, data));
            }
            budget.expanded = expanded;
            let parsed: Vec<Result<InputClass>> = rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()?
                .install(|| {
                    pending
                        .into_par_iter()
                        .map(|(name, data)| {
                            let id = hash(&data);
                            let class = ClassFile::parse_bounded(data, limits.expanded_bytes)
                                .with_context(|| name.clone())?;
                            Ok(InputClass {
                                entry: Some(name),
                                source_id: id,
                                class,
                            })
                        })
                        .collect()
                });
            for result in parsed {
                let item = result?;
                let class = &item.class;
                let name = item.entry.as_ref().unwrap();
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
                classes.push(item);
            }
            budget.metadata = budget
                .metadata
                .checked_add(metadata_bytes)
                .context("RESOURCE_LIMIT: metadata allocation overflow")?;
            ensure!(
                budget.metadata <= limits.expanded_bytes,
                "RESOURCE_LIMIT: archive metadata budget exceeded"
            );
            for (index, name) in nested {
                ensure!(
                    !name.contains('!'),
                    "INVALID_INPUT: ambiguous nested archive name"
                );
                let entry = zip.by_index(index)?;
                let size = usize::try_from(entry.size())
                    .context("RESOURCE_LIMIT: nested archive size overflow")?;
                budget.expanded = budget
                    .expanded
                    .checked_add(size)
                    .context("RESOURCE_LIMIT: expanded size overflow")?;
                ensure!(
                    budget.expanded <= limits.expanded_bytes,
                    "RESOURCE_LIMIT: expanded byte budget exceeded"
                );
                let mut data = Vec::new();
                entry.take(size as u64 + 1).read_to_end(&mut data)?;
                ensure!(
                    data.len() == size,
                    "INVALID_INPUT: inconsistent nested archive size"
                );
                let inner =
                    Self::read_bytes(data, Path::new(&name), limits, release, depth + 1, budget)
                        .with_context(|| name.clone())?;
                for mut item in inner.classes {
                    item.entry = Some(format!("{name}!{}", item.entry.unwrap_or_default()));
                    classes.push(item);
                }
                for mut entry in inner.entries {
                    entry.name = format!("{name}!{}", entry.name);
                    entries.push(entry);
                }
            }
            ensure!(
                depth != 0 || !classes.is_empty(),
                "INVALID_INPUT: archive contains no selected JVM class files"
            );
        }
        Ok(Self {
            source_id,
            file_bytes,
            classes,
            entries,
        })
    }

    pub fn matches_class(class: &InputClass, name: &str) -> bool {
        let (archive, local) = name
            .rsplit_once('!')
            .map_or((None, name), |(archive, local)| (Some(archive), local));
        class.class.name == normalize_class_name(local)
            && archive.is_none_or(|archive| {
                class
                    .entry
                    .as_deref()
                    .and_then(|entry| entry.rsplit_once('!'))
                    .is_some_and(|(prefix, _)| prefix == archive)
            })
    }

    pub fn class_at(class: &InputClass) -> String {
        let prefix = class
            .entry
            .as_deref()
            .and_then(|entry| entry.rsplit_once('!'))
            .map(|(archive, _)| format!("{archive}!"))
            .unwrap_or_default();
        format!("{prefix}L{};", class.class.name)
    }

    /// Method coordinate: Lclass;->name(proto), including return type.
    pub fn method_at(class: &InputClass, index: usize) -> String {
        let method = &class.class.methods[index];
        format!(
            "{}->{}{}",
            Self::class_at(class),
            method.name,
            method.descriptor
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
        let selected = self.class_by_name(class_name)?;
        for class in std::iter::once(selected) {
            for (index, method) in class.class.methods.iter().enumerate() {
                if method.name == name && method.descriptor == descriptor {
                    return Ok((class, index));
                }
            }
        }
        // Well formed and absent: the caller re-reads a listing, it does not fix the query.
        Err(rxd_core::Error::failure(
            rxd_core::Failure::NotFound,
            format!("no method {coordinate} in this input"),
        )
        .into())
    }

    /// Select exactly one class by its internal, dotted or descriptor name.
    pub fn class_by_name(&self, name: &str) -> Result<&InputClass> {
        let mut hits = self
            .classes
            .iter()
            .filter(|class| Self::matches_class(class, name));
        let selected = hits.next().ok_or_else(|| {
            rxd_core::Error::failure(
                rxd_core::Failure::NotFound,
                format!("no class {name} in this input"),
            )
        })?;
        // Unqualified names must remain unambiguous across nested archives.
        ensure!(
            hits.next().is_none(),
            "INVALID_QUERY: class {name} is defined more than once"
        );
        Ok(selected)
    }
}

/// Normalize dotted/descriptive class names to internal slash form.
pub fn normalize_class_name(name: &str) -> String {
    name.strip_prefix('L')
        .and_then(|rest| rest.strip_suffix(';'))
        .unwrap_or(name)
        .replace('.', "/")
}

fn nested_entries(
    zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    limits: Limits,
    depth: usize,
    budget: &mut ArchiveBudget,
) -> Result<Vec<InputEntry>> {
    let mut result = Vec::new();
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".jar") || lower.ends_with(".aar") || lower.ends_with(".zip")) {
            continue;
        }
        ensure!(depth < 16, "RESOURCE_LIMIT: nested archive depth exceeded");
        ensure!(
            !name.contains('!'),
            "INVALID_INPUT: ambiguous nested archive name"
        );
        let size = usize::try_from(entry.size())
            .context("RESOURCE_LIMIT: nested archive size overflow")?;
        budget.expanded = budget
            .expanded
            .checked_add(size)
            .context("RESOURCE_LIMIT: expanded size overflow")?;
        ensure!(
            budget.expanded <= limits.expanded_bytes,
            "RESOURCE_LIMIT: expanded byte budget exceeded"
        );
        let mut data = Vec::new();
        entry.take(size as u64 + 1).read_to_end(&mut data)?;
        ensure!(
            data.len() == size,
            "INVALID_INPUT: inconsistent nested archive size"
        );
        check_directory(&data, limits.entries)?;
        let mut inner = zip::ZipArchive::new(Cursor::new(data))?;
        budget.entries = budget
            .entries
            .checked_add(inner.len())
            .context("RESOURCE_LIMIT: entry count overflow")?;
        ensure!(
            budget.entries <= limits.entries,
            "RESOURCE_LIMIT: archive entry budget exceeded"
        );
        for i in 0..inner.len() {
            let entry = inner.by_index(i)?;
            result.push(InputEntry {
                name: format!("{name}!{}", entry.name()),
                compression: match entry.compression() {
                    zip::CompressionMethod::Stored => 0,
                    zip::CompressionMethod::Deflated => 8,
                    _ => 255,
                },
                compressed_size: usize::try_from(entry.compressed_size())?,
                uncompressed_size: usize::try_from(entry.size())?,
            });
        }
        for mut entry in nested_entries(&mut inner, limits, depth + 1, budget)? {
            entry.name = format!("{name}!{}", entry.name);
            result.push(entry);
        }
    }
    Ok(result)
}

fn hash(bytes: &[u8]) -> String {
    rxd_core::identity::input_id(bytes)
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
