//! ZIP discovery, DEX views and parallel entry analysis.
//! Results retain central-directory order.

use crate::archive::{ZipEntry, inflate_entry};
use crate::dex;
use crate::query::Query;
use anyhow::{Context, Result, bail};
use memmap2::Mmap;
use rayon::prelude::*;
use std::cell::OnceCell;
use std::fs::File;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

enum Archive {
    Mapped(Mmap),
}

/// Immutable limits applied while reading one archive command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchivePolicy {
    pub max_inflated_entry: Option<usize>,
}

impl Default for ArchivePolicy {
    fn default() -> Self {
        Self {
            max_inflated_entry: Some(crate::archive::DEFAULT_MAX_INFLATED_ENTRY),
        }
    }
}

impl Archive {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the mapping is owned by this value and therefore cannot outlive the file.
        Ok(Archive::Mapped(
            unsafe { Mmap::map(&file) }.context("map APK")?,
        ))
    }
}

impl crate::archive::BytesSource for Archive {
    fn source_len(&self) -> usize {
        match self {
            Archive::Mapped(mapped) => mapped.len(),
        }
    }

    fn range(&self, offset: usize, len: usize) -> Result<std::borrow::Cow<'_, [u8]>> {
        let end = offset.checked_add(len).context("range overflow")?;
        if end > self.source_len() {
            bail!("range out of bounds");
        }
        match self {
            Archive::Mapped(mapped) => Ok(std::borrow::Cow::Borrowed(&mapped[offset..end])),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClassEntry {
    pub descriptor: String,
    /// None for Object or absent superclass.
    pub super_descriptor: Option<String>,
    /// Shared entry name for all classes in one DEX.
    pub dex_name: std::sync::Arc<str>,
    /// Big-endian descriptor prefix for inline comparisons.
    key: u64,
}

/// Eight-byte prefix key; equal keys fall back to full descriptor comparison.
const KEY_BYTES: usize = 8;

/// Zero padding preserves prefix ordering because descriptors contain no NUL.
fn descriptor_key(descriptor: &str) -> u64 {
    let bytes = descriptor.as_bytes();
    let take = bytes.len().min(KEY_BYTES);
    let mut key = [0u8; KEY_BYTES];
    key[..take].copy_from_slice(&bytes[..take]);
    u64::from_be_bytes(key)
}

impl ClassEntry {
    pub fn new(
        descriptor: String,
        super_descriptor: Option<String>,
        dex_name: std::sync::Arc<str>,
    ) -> Self {
        let key = descriptor_key(&descriptor);
        Self {
            descriptor,
            super_descriptor,
            dex_name,
            key,
        }
    }
}

impl Ord for ClassEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.descriptor.cmp(&other.descriptor))
            .then_with(|| self.dex_name.cmp(&other.dex_name))
    }
}

impl PartialOrd for ClassEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ClassEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for ClassEntry {}

impl ClassEntry {
    pub fn java_name(&self) -> &str {
        self.descriptor
            .strip_prefix('L')
            .and_then(|name| name.strip_suffix(';'))
            .unwrap_or(&self.descriptor)
    }

    pub fn package(&self) -> &str {
        self.java_name()
            .rsplit_once('/')
            .map_or("", |(package, _)| package)
    }

    pub fn simple_name(&self) -> &str {
        self.java_name()
            .rsplit_once('/')
            .map_or(self.java_name(), |(_, name)| name)
    }
}

/// Lazy entry inflation; prefix probes avoid reading the code section.
struct InflatedDex<'a> {
    entry: &'a ZipEntry,
    source: &'a Archive,
    started: Instant,

    max_inflated: Option<usize>,
    data: OnceCell<(Vec<u8>, Duration)>,
}

impl InflatedDex<'_> {
    fn data(&self) -> Result<&[u8]> {
        if self.data.get().is_none() {
            let started = Instant::now();
            let bytes = crate::profile::measure(crate::profile::Stage::Inflate, || {
                inflate_entry(self.source, self.entry, self.max_inflated)
            })?;
            let _ = self.data.set((bytes, started.elapsed()));
        }
        Ok(&self.data.get().expect("filled above").0)
    }

    fn inflate_elapsed(&self) -> Duration {
        self.data
            .get()
            .map_or(Duration::ZERO, |(_, elapsed)| *elapsed)
    }

    fn read_prefix(&self, aim: usize) -> Result<Vec<u8>> {
        crate::archive::inflate_prefix(self.source, self.entry, self.max_inflated, aim)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EntryOrder {
    Natural,

    SmallestFirst,
}

/// Map root DEX entries in parallel, returning central-directory order.
/// Early stop skips unstarted entries, not in-flight work.
fn map_dex_entries<T: Send>(
    path: &Path,
    policy: ArchivePolicy,
    threads: usize,
    order: EntryOrder,
    stop: Option<&AtomicBool>,
    map: impl for<'a> Fn(InflatedDex<'a>) -> Result<Vec<T>> + Sync,
) -> Result<Vec<T>> {
    if threads == 0 {
        bail!("worker count must be greater than zero");
    }
    let archive = crate::profile::measure(crate::profile::Stage::Map, || Archive::open(path))?;
    let mut entries = crate::profile::measure(crate::profile::Stage::ZipIndex, || {
        parse_dex_entries(&archive)
    })?;
    crate::profile::count(crate::profile::Counter::Entries, entries.len() as u64);
    crate::profile::measure(crate::profile::Stage::Select, || match order {
        EntryOrder::Natural => {}
        EntryOrder::SmallestFirst => entries.sort_by_key(|entry| entry.compressed_size),
    });
    // One entry, walked the same way whether the caller drives it in parallel or not.
    let max_inflated = policy.max_inflated_entry;
    let run_entry = |entry: &ZipEntry| -> Result<Vec<T>> {
        if stop.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Ok(Vec::new());
        }
        map(InflatedDex {
            entry,
            source: &archive,
            started: Instant::now(),
            max_inflated,
            data: OnceCell::new(),
        })
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    let results: Vec<Result<Vec<T>>> = pool.install(|| entries.par_iter().map(run_entry).collect());
    let mut out = Vec::new();
    for result in results {
        out.extend(result?);
    }
    Ok(out)
}

/// Field order preserves rendered reference-hit sort order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReferenceHit {
    pub dex_name: String,
    pub member: String,

    pub matched: String,
}

impl ReferenceHit {
    pub fn render(&self) -> String {
        let mut line = String::with_capacity(
            self.dex_name.len() + self.member.len() + self.matched.len() + 24,
        );
        line.push_str(&self.dex_name);
        line.push_str(" | ");
        line.push_str(&self.member);
        line.push_str(" | matched=(");
        line.push_str(&self.matched);
        line.push(')');
        line
    }
}

/// Collect unsorted reference hits; the caller chooses final output order.
#[cfg(test)]
pub fn find_reference_hits(
    path: &Path,
    query: &Query,
    threads: usize,
    debug: bool,
) -> Result<Vec<ReferenceHit>> {
    find_reference_hits_with_policy(path, ArchivePolicy::default(), query, threads, debug)
}

pub fn find_reference_hits_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    query: &Query,
    threads: usize,
    debug: bool,
) -> Result<Vec<ReferenceHit>> {
    // One probe decides the prefix policy for the archive, for the pre-check below.
    let prefix_policy: OnceLock<PrefixPolicy> = OnceLock::new();
    // Prefix prechecks pay only for class-constrained queries.
    let needs_targets = query.bounded_by_class();
    // An anchor names one identity; "no entry resolved it" is a different answer from "no
    // reference points at it", and only the entries themselves can tell them apart.
    let anchor_resolved = std::sync::atomic::AtomicBool::new(false);
    let rows = map_dex_entries(
        path,
        policy,
        threads,
        EntryOrder::SmallestFirst,
        None,
        |inflated| {
            if needs_targets
                && let Some(prefix) = prefix_probe(&inflated, &prefix_policy)?
                && dex::prefix_has_targets(&prefix, query)? == Some(false)
            {
                if debug {
                    crate::diag::diagnose(format_args!(
                        "[APK] '{}' skipped (no target in prefix) total={:.2} us",
                        inflated.entry.name,
                        inflated.started.elapsed().as_secs_f64() * 1_000_000.0
                    ));
                }
                return Ok(Vec::new());
            }
            let mut rows = Vec::new();
            for logical in dex::container::logical_dexes(&inflated.entry.name, inflated.data()?)? {
                let (found, resolved) = dex::find_references_with_anchor(&logical.data, query)?;
                anchor_resolved.fetch_or(resolved, std::sync::atomic::Ordering::Relaxed);
                for row in found {
                    rows.push(ReferenceHit {
                        dex_name: logical.name.clone(),
                        member: row.member,
                        matched: row.matched,
                    });
                }
            }
            if debug {
                crate::diag::diagnose(format_args!(
                    "[APK] '{}' inflate={:.2} us process={:.2} us",
                    inflated.entry.name,
                    inflated.inflate_elapsed().as_secs_f64() * 1_000_000.0,
                    (inflated.started.elapsed() - inflated.inflate_elapsed()).as_secs_f64()
                        * 1_000_000.0
                ));
            }
            Ok(rows)
        },
    )?;
    if query.is_anchor() && !anchor_resolved.load(std::sync::atomic::Ordering::Relaxed) {
        bail!("{}", query.anchor_not_found());
    }
    Ok(rows)
}

#[cfg(test)]
pub fn list_classes(path: &Path, threads: usize, debug: bool) -> Result<Vec<ClassEntry>> {
    list_classes_with_policy(path, ArchivePolicy::default(), threads, debug)
}

/// Split `Lx/y;->name(parameters)return` without dropping the return descriptor.
pub fn parse_method_coordinate(coordinate: &str) -> Result<(String, String, String)> {
    let malformed = || {
        rxd_core::Error::failure(
            rxd_core::Failure::Query,
            format!("{coordinate} is not `Lclass;->name(proto)`"),
        )
    };
    let (class, rest) = coordinate.split_once("->").ok_or_else(malformed)?;
    let open = rest.find('(').ok_or_else(malformed)?;
    let (name, prototype) = rest.split_at(open);
    // A DEX prototype ends in its return type, not ')'. Consume descriptors
    // iteratively so malformed/trailing types cannot masquerade as missing members.
    fn field_tail(mut value: &str) -> Option<&str> {
        let mut dimensions = 0;
        while let Some(tail) = value.strip_prefix('[') {
            dimensions += 1;
            if dimensions > 255 {
                return None;
            }
            value = tail;
        }
        match *value.as_bytes().first()? {
            b'B' | b'C' | b'D' | b'F' | b'I' | b'J' | b'S' | b'Z' => Some(&value[1..]),
            b'L' => {
                let (name, tail) = value[1..].split_once(';')?;
                if name.is_empty() || name.contains(['[', '(', ')']) {
                    return None;
                }
                Some(tail)
            }
            _ => None,
        }
    }
    let (mut parameters, returns) = prototype[1..].split_once(')').ok_or_else(malformed)?;
    if class.is_empty() || name.is_empty() || !(returns == "V" || field_tail(returns) == Some("")) {
        return Err(malformed().into());
    }
    while !parameters.is_empty() {
        parameters = field_tail(parameters).ok_or_else(malformed)?;
    }
    Ok((class.to_owned(), name.to_owned(), prototype.to_owned()))
}

/// Render the selected method from the first entry defining its class.
pub fn decompile_method_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    class: &str,
    name: &str,
    prototype: &str,
    threads: usize,
    debug: bool,
) -> Result<String> {
    let hit = map_class_hit(path, policy, class, threads, debug, |data| {
        crate::emitter::render_method(data, class, name, prototype)?.with_context(|| {
            // The class exists and the member does not: absence, not a bad query.
            rxd_core::Error::failure(
                rxd_core::Failure::NotFound,
                format!("no method {class}->{name}{prototype} in this input"),
            )
        })
    })?;
    hit.map(|(_, source)| source).ok_or_else(|| {
        rxd_core::Error::failure(
            rxd_core::Failure::NotFound,
            format!("no class {class} in this input"),
        )
        .into()
    })
}

pub use crate::dex::methods::Selector as MethodSelector;

/// List declared fields in central-directory order.
pub fn list_fields_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    selector: &MethodSelector<'_>,
    threads: usize,
    debug: bool,
) -> Result<(Vec<String>, usize)> {
    let started = Instant::now();
    let chunks = map_dex_entries(
        path,
        policy,
        threads,
        EntryOrder::Natural,
        None,
        |inflated| {
            let mut out = String::new();
            let mut rows = 0;
            for logical in dex::container::logical_dexes(&inflated.entry.name, inflated.data()?)? {
                rows += crate::dex::methods::render_fields(&logical.data, selector, &mut out)?;
            }
            Ok(vec![(out, rows)])
        },
    )?;
    let mut records = 0usize;
    let mut bytes = 0usize;
    let mut payloads = Vec::with_capacity(chunks.len());
    for (chunk, count) in chunks {
        bytes += chunk.len();
        records += count;
        payloads.push(chunk);
    }
    if debug {
        crate::diag::diagnose(format_args!(
            "[fields] records={} rendered={:.2} ms bytes={}",
            records,
            started.elapsed().as_secs_f64() * 1e3,
            bytes
        ));
    }
    Ok((payloads, records))
}

pub fn list_methods_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    selector: &MethodSelector<'_>,
    threads: usize,
    debug: bool,
) -> Result<(Vec<String>, usize)> {
    let started = Instant::now();
    let chunks = map_dex_entries(
        path,
        policy,
        threads,
        EntryOrder::Natural,
        None,
        |inflated| {
            let mut out = String::new();
            let mut rows = 0;
            for logical in dex::container::logical_dexes(&inflated.entry.name, inflated.data()?)? {
                rows += crate::dex::methods::render_methods(&logical.data, selector, &mut out)?;
            }
            Ok(vec![(out, rows)])
        },
    )?;
    let mut records = 0usize;
    let mut bytes = 0usize;
    let mut payloads = Vec::with_capacity(chunks.len());
    for (chunk, count) in chunks {
        bytes += chunk.len();
        records += count;
        payloads.push(chunk);
    }
    if debug {
        crate::diag::diagnose(format_args!(
            "[methods] records={} rendered={:.2} ms bytes={}",
            records,
            started.elapsed().as_secs_f64() * 1e3,
            bytes
        ));
    }
    Ok((payloads, records))
}

pub fn list_classes_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    threads: usize,
    debug: bool,
) -> Result<Vec<ClassEntry>> {
    let started = Instant::now();
    // One probe decides the prefix policy for the whole archive (see PrefixPolicy).
    let prefix_policy: OnceLock<PrefixPolicy> = OnceLock::new();
    let classes = map_dex_entries(
        path,
        policy,
        threads,
        EntryOrder::Natural,
        None,
        |inflated| {
            if let Some(entries) = prefix_class_entries(&inflated, &prefix_policy)? {
                return Ok(entries);
            }
            let mut classes = Vec::new();
            for logical in dex::container::logical_dexes(&inflated.entry.name, inflated.data()?)? {
                for (descriptor, super_descriptor) in dex::class_names(&logical.data)? {
                    classes.push(ClassEntry::new(
                        descriptor,
                        super_descriptor,
                        logical.name.as_str().into(),
                    ));
                }
            }
            Ok(classes)
        },
    )?;
    let extracted = started.elapsed();
    if debug {
        crate::diag::diagnose(format_args!(
            "[classes] entries={} extract={:.2} ms",
            classes.len(),
            extracted.as_secs_f64() * 1e3
        ));
    }
    Ok(classes)
}

/// One late-string-layout probe disables subsequent prefix probes.
#[derive(Clone, Copy)]
struct PrefixPolicy {
    worth_it: bool,
}

/// Probe a class from prefix bytes; `None` requires the full reader.
fn prefix_defines_class(
    inflated: &InflatedDex<'_>,
    descriptor: &[u8],
    policy: &OnceLock<PrefixPolicy>,
) -> Result<Option<bool>> {
    let Some(prefix) = prefix_probe(inflated, policy)? else {
        return Ok(None);
    };
    dex::prefix::defines_class(&prefix, descriptor)
}

/// Probe header, string offsets, then string data.
/// Return `None` when completeness cannot be established.
fn prefix_probe(
    inflated: &InflatedDex<'_>,
    policy: &OnceLock<PrefixPolicy>,
) -> Result<Option<Vec<u8>>> {
    const MIN_GAIN: usize = 256 * 1024;
    /// Bytes kept beyond the last string offset, for the string's own data.
    const SLACK: usize = 64 * 1024;
    /// A prefix is only worth a second read below this share of the entry.
    const WORTH_BELOW: f64 = 0.70;
    /// Enough of the header to name every id table (the DEX header is 0x70 bytes).
    const HEADER_PROBE: usize = 8 * 1024;

    let size = inflated.entry.uncompressed_size;
    if inflated.entry.compression != 8 || size <= MIN_GAIN {
        return Ok(None);
    }
    if let Some(known) = policy.get()
        && !known.worth_it
    {
        return Ok(None);
    }
    // A 041 container overlays a header per member and needs the whole address space,
    // so the full path keeps handling those.
    let header = inflated.read_prefix(HEADER_PROBE)?;
    if header.get(..8) == Some(b"dex\n041\0") {
        return Ok(None);
    }
    let Some(strings_end) = dex::prefix::string_ids_end(&header) else {
        return Ok(None);
    };
    if strings_end > size {
        return Ok(None);
    }
    let offsets = inflated.read_prefix(strings_end + SLACK)?;
    // An incomplete table leaves archive-wide prefix policy undecided.
    let Some(needed) = dex::prefix::string_data_end(&offsets) else {
        return Ok(None);
    };
    let fraction = needed as f64 / size as f64;
    let worth_it = fraction < WORTH_BELOW;
    let _ = policy.set(PrefixPolicy { worth_it });
    if !worth_it {
        return Ok(None);
    }
    Ok(Some(inflated.read_prefix(needed + SLACK)?))
}

/// Return classes only from a complete prefix; otherwise require full inflation.
fn prefix_class_entries(
    inflated: &InflatedDex<'_>,
    policy: &OnceLock<PrefixPolicy>,
) -> Result<Option<Vec<ClassEntry>>> {
    let Some(prefix) = prefix_probe(inflated, policy)? else {
        return Ok(None);
    };
    let Some(names) = dex::prefix::class_names(&prefix)? else {
        return Ok(None);
    };
    let dex_name: std::sync::Arc<str> = inflated.entry.name.as_str().into();
    Ok(Some(
        names
            .into_iter()
            .map(|(descriptor, super_descriptor)| {
                ClassEntry::new(
                    descriptor,
                    super_descriptor,
                    std::sync::Arc::clone(&dex_name),
                )
            })
            .collect(),
    ))
}

/// Visit the defining entry while mapped; callers must copy retained bytes.
fn map_class_hit<T: Send>(
    path: &Path,
    policy: ArchivePolicy,
    descriptor: &str,
    threads: usize,
    debug: bool,
    map: impl for<'a> Fn(&'a [u8]) -> Result<T> + Sync,
) -> Result<Option<(String, T)>> {
    let stop = AtomicBool::new(false);
    // One probe decides the prefix policy for the archive (see PrefixPolicy).
    let prefix_policy: OnceLock<PrefixPolicy> = OnceLock::new();
    let hits = map_dex_entries(
        path,
        policy,
        threads,
        EntryOrder::SmallestFirst,
        Some(&stop),
        |inflated| {
            // Entries that do not define the class only need the prefix, so the code
            // section is never decompressed for them. `None` means "cannot tell".
            match prefix_defines_class(&inflated, descriptor.as_bytes(), &prefix_policy)? {
                Some(false) => {
                    if debug {
                        crate::diag::diagnose(format_args!(
                            "[APK] '{}' hit=false total={:.2} us",
                            inflated.entry.name,
                            inflated.started.elapsed().as_secs_f64() * 1_000_000.0
                        ));
                    }
                    return Ok(Vec::new());
                }
                Some(true) => {
                    stop.store(true, Ordering::Relaxed);
                    if debug {
                        crate::diag::diagnose(format_args!(
                            "[APK] '{}' hit=true total={:.2} us",
                            inflated.entry.name,
                            inflated.started.elapsed().as_secs_f64() * 1_000_000.0
                        ));
                    }
                    return Ok(vec![(inflated.entry.name.clone(), map(inflated.data()?)?)]);
                }
                None => {}
            }
            for logical in dex::container::logical_dexes(&inflated.entry.name, inflated.data()?)? {
                if dex::defines_class(&logical.data, descriptor.as_bytes())? {
                    stop.store(true, Ordering::Relaxed);
                    if debug {
                        crate::diag::diagnose(format_args!(
                            "[APK] '{}' hit=true total={:.2} us",
                            inflated.entry.name,
                            inflated.started.elapsed().as_secs_f64() * 1_000_000.0
                        ));
                    }
                    return Ok(vec![(logical.name, map(&logical.data)?)]);
                }
            }
            if debug {
                crate::diag::diagnose(format_args!(
                    "[APK] '{}' hit=false total={:.2} us",
                    inflated.entry.name,
                    inflated.started.elapsed().as_secs_f64() * 1_000_000.0
                ));
            }
            Ok(Vec::new())
        },
    )?;
    Ok(hits.into_iter().next())
}

/// Render a class with its entry name; None if no DEX defines it.
#[cfg(test)]
pub fn decompile_class(
    path: &Path,
    descriptor: &str,
    threads: usize,
    debug: bool,
) -> Result<Option<(String, String)>> {
    decompile_class_with_policy(path, ArchivePolicy::default(), descriptor, threads, debug)
}

pub fn decompile_class_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    descriptor: &str,
    threads: usize,
    debug: bool,
) -> Result<Option<(String, String)>> {
    map_class_hit(path, policy, descriptor, threads, debug, |entry| {
        // DEX 041 containers carry an extra-long header and a container-wide
        // checksum, which the decompiler rejects; hand it a normalized view.
        let normalized = dex::container::standard_header_view(entry);
        let data = normalized.as_deref().unwrap_or(entry);
        crate::emitter::render(data, descriptor)
    })
}

/// Match ASCII directly in MUTF-8; non-ASCII pairs require decoding.
fn raw_contains_ignore_case(raw: &[u8], needle_lower: &str) -> Option<bool> {
    if !needle_lower.is_ascii() || !raw.is_ascii() {
        return None;
    }
    let needle = needle_lower.as_bytes();
    if needle.is_empty() {
        return Some(true);
    }
    if needle.len() > raw.len() {
        return Some(false);
    }
    Some(raw.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.to_ascii_lowercase() == *right)
    }))
}

#[derive(Clone, Debug)]
pub struct StringEntry {
    pub dex_name: std::sync::Arc<str>,
    /// DEX string-table index, preserving table order.
    pub index: usize,
    pub value: String,
}

/// List matching strings in entry/table order; offset and limit select a page.
#[cfg(test)]
pub fn list_strings(
    path: &Path,
    threads: usize,
    debug: bool,
    filter: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<StringEntry>> {
    let filter = filter.map(|needle| rxd_core::filter::Contains::new(needle, false));
    list_strings_with_policy(
        path,
        ArchivePolicy::default(),
        threads,
        debug,
        filter.as_ref(),
        limit,
        offset,
    )
}

pub fn list_strings_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    threads: usize,
    debug: bool,
    filter: Option<&rxd_core::filter::Contains>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<StringEntry>> {
    if threads == 0 {
        bail!("worker count must be greater than zero");
    }
    let take = limit.unwrap_or(usize::MAX);
    let skip = offset.unwrap_or(0);

    let fill = skip.saturating_add(take);
    // Only serial scans can stop early without changing which page wins.
    let stop_early = threads == 1;

    let found = std::sync::atomic::AtomicUsize::new(0);
    let started = Instant::now();
    let rows: Vec<StringEntry> = map_dex_entries(
        path,
        policy,
        threads,
        EntryOrder::Natural,
        None,
        |inflated| {
            let stop = std::sync::atomic::Ordering::Relaxed;
            // Check page completion before inflating the next entry.
            if stop_early && found.load(stop) >= fill {
                return Ok(Vec::new());
            }
            let dex_name: std::sync::Arc<str> = inflated.entry.name.as_str().into();
            let mut rows = Vec::new();
            for logical in dex::container::logical_dexes(&inflated.entry.name, inflated.data()?)? {
                if stop_early && found.load(stop) >= fill {
                    break;
                }
                dex::for_each_string(&logical.data, |index, raw| {
                    if stop_early && found.load(stop) >= fill {
                        return Ok(false);
                    }
                    let matches = match filter {
                        None => true,
                        // An ASCII needle against ASCII bytes is a comparison, not a decode;
                        // anything else takes the Unicode fold.
                        Some(filter) => match filter
                            .ascii_needle()
                            .and_then(|needle| raw_contains_ignore_case(raw, needle))
                        {
                            Some(found) => found,
                            None => filter.matches(&dex::decode_string(raw)),
                        },
                    };
                    if matches {
                        found.fetch_add(1, stop);
                        rows.push(StringEntry {
                            dex_name: std::sync::Arc::clone(&dex_name),
                            index,
                            value: dex::decode_string(raw),
                        });
                    }
                    Ok(true)
                })?;
            }
            Ok(rows)
        },
    )?;
    let mut rows = rows;
    if skip > 0 {
        rows.drain(..skip.min(rows.len()));
    }
    rows.truncate(take);
    if debug {
        crate::diag::diagnose(format_args!(
            "[strings] rows={} total={:.2} ms",
            rows.len(),
            started.elapsed().as_secs_f64() * 1e3
        ));
    }
    Ok(rows)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// A runtime field index: `ifields_` first, then `sfields_`.
    Field,
    /// A runtime method index: the `method_ids` index.
    Method,
}

/// List physical entries in central-directory order, including duplicate names.
pub fn list_entries(path: &Path) -> Result<Vec<ZipEntry>> {
    let archive = Archive::open(path)?;
    if is_bare_dex(&archive) {
        // `BytesSource` is the archive's byte-count, and every build implements it.
        return Ok(vec![ZipEntry::bare(
            "classes.dex",
            crate::archive::BytesSource::source_len(&archive),
        )]);
    }
    crate::archive::parse_zip_entries(&archive, |_| true)
}

/// Visit selected original payloads in archive order; reject duplicate names.
pub fn extract_entries_with_policy(
    path: &Path,
    policy: ArchivePolicy,
    names: &[String],
    mut visit: impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<()> {
    use crate::profile::{Stage, measure};
    let archive = measure(Stage::Map, || Archive::open(path))?;
    let entries = if is_bare_dex(&archive) {
        vec![ZipEntry::bare(
            "classes.dex",
            crate::archive::BytesSource::source_len(&archive),
        )]
    } else {
        crate::archive::parse_zip_entries(&archive, |_| true)?
    };
    let mut found = vec![false; names.len()];
    for entry in &entries {
        let Some(index) = names.iter().position(|name| name == &entry.name) else {
            continue;
        };
        if std::mem::replace(&mut found[index], true) {
            bail!("INVALID_QUERY: entry {} appears more than once", entry.name);
        }
    }
    if let Some(index) = found.iter().position(|seen| !seen) {
        bail!("NOT_FOUND: entry {} not found", names[index]);
    }
    for entry in &entries {
        if names.iter().any(|name| name == &entry.name) {
            let bytes = measure(Stage::Inflate, || {
                inflate_entry(&archive, entry, policy.max_inflated_entry)
            })?;
            visit(&entry.name, &bytes)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn read_entry(path: &Path, wanted: &str) -> Result<Vec<u8>> {
    read_entry_with_policy(path, ArchivePolicy::default(), wanted)
}

pub fn read_entry_with_policy(path: &Path, policy: ArchivePolicy, wanted: &str) -> Result<Vec<u8>> {
    use crate::profile::{Stage, measure};
    let archive = measure(Stage::Map, || Archive::open(path))?;
    // Match zipfile: the last AndroidManifest.xml record wins.
    let entry = crate::archive::parse_zip_entries(&archive, |name| name == wanted.as_bytes())?
        .into_iter()
        .last()
        .with_context(|| format!("{wanted} not found in APK"))?;
    inflate_entry(&archive, &entry, policy.max_inflated_entry)
}

pub(crate) fn is_bare_dex<S: crate::archive::BytesSource + ?Sized>(source: &S) -> bool {
    let Ok(head) = source.range(0, 8) else {
        return false;
    };
    head.len() == 8
        && head.starts_with(b"dex\n")
        && head[4..7].iter().all(u8::is_ascii_digit)
        && head[7] == 0
}

/// Prefer root classes*.dex, then any root *.dex; bare DEX is classes.dex.
fn parse_dex_entries<S: crate::archive::BytesSource + ?Sized>(source: &S) -> Result<Vec<ZipEntry>> {
    if is_bare_dex(source) {
        return Ok(vec![ZipEntry::bare("classes.dex", source.source_len())]);
    }
    let named = crate::archive::parse_zip_entries(source, |name| {
        name.starts_with(b"classes") && name.ends_with(b".dex") && !name.contains(&b'/')
    })?;
    if !named.is_empty() {
        return Ok(dedup_names(named));
    }
    Ok(dedup_names(crate::archive::parse_zip_entries(
        source,
        |name| name.ends_with(b".dex") && !name.contains(&b'/'),
    )?))
}

/// Keep the first DEX entry per name, matching the reference reader.
fn dedup_names(entries: Vec<crate::archive::ZipEntry>) -> Vec<crate::archive::ZipEntry> {
    let mut seen = std::collections::HashSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(entry.name.clone()))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{build_zip, temp_apk};
    use crate::query::Pattern;

    fn find_references(
        path: &Path,
        query: &Query,
        threads: usize,
        debug: bool,
    ) -> Result<Vec<String>> {
        Ok(find_reference_hits(path, query, threads, debug)?
            .iter()
            .map(ReferenceHit::render)
            .collect())
    }

    #[test]
    fn dex_entries_fall_back_to_any_root_dex_name() {
        let dex = dex::tests::const_string_fixture(1);
        let zip = build_zip(&[("app.dex", &dex, true), ("assets/other.dex", &dex, false)]);
        let path = temp_apk("fallback", &zip);
        let rows = find_references(
            &path,
            &Query::String(Pattern::search("Authorization", false)),
            2,
            false,
        )
        .unwrap();
        assert_eq!(
            rows,
            ["app.dex | LFixture0;->m0()V | matched=(Authorization)"]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn duplicate_classes_dex_entries_use_the_first_copy() {
        let first = dex::tests::const_string_fixture(1);
        let second = dex::tests::const_string_fixture(2);
        let zip = build_zip(&[
            ("classes.dex", &first, true),
            ("classes.dex", &second, true),
        ]);
        let path = temp_apk("duplicate-classes", &zip);
        let rows = find_references(
            &path,
            &Query::String(Pattern::search("Authorization", false)),
            2,
            false,
        )
        .unwrap();
        assert_eq!(
            rows,
            ["classes.dex | LFixture0;->m0()V | matched=(Authorization)"]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn duplicate_fallback_dex_entries_use_the_first_copy() {
        let first = dex::tests::const_string_fixture(1);
        let second = dex::tests::const_string_fixture(2);
        let zip = build_zip(&[("app.dex", &first, true), ("app.dex", &second, true)]);
        let path = temp_apk("duplicate-fallback", &zip);
        let rows = find_references(
            &path,
            &Query::String(Pattern::search("Authorization", false)),
            2,
            false,
        )
        .unwrap();
        assert_eq!(
            rows,
            ["app.dex | LFixture0;->m0()V | matched=(Authorization)"]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_filter_ignores_case_without_changing_the_answer() {
        for (haystack, needle, expected) in [
            ("Authorization", "auth", true),
            ("Authorization", "AUTH", true),
            ("Authorization", "xyz", false),
            ("", "", true),
            ("short", "much longer needle", false),
            // Non-ASCII takes the Unicode fold, which is what a JS `toLowerCase` did.
            ("Grüße", "GRÜSSE", false),
            ("Grüße", "grüße", true),
            ("ключ 🔑", "КЛЮЧ", true),
            ("漢字", "漢", true),
        ] {
            assert_eq!(
                rxd_core::filter::Contains::new(needle, false).matches(haystack),
                expected,
                "{haystack:?} / {needle:?}"
            );
        }
    }

    #[test]
    fn the_raw_filter_defers_anything_it_cannot_answer() {
        // ASCII on both sides is a comparison.
        assert_eq!(
            raw_contains_ignore_case(b"Authorization header", "auth"),
            Some(true)
        );
        assert_eq!(
            raw_contains_ignore_case(b"Authorization header", "zzz"),
            Some(false)
        );
        assert_eq!(raw_contains_ignore_case(b"anything", ""), Some(true));
        assert_eq!(
            raw_contains_ignore_case(b"short", "much longer needle"),
            Some(false)
        );
        // Anything else is the caller's to decode and fold, never a guess.
        assert_eq!(raw_contains_ignore_case("ключ".as_bytes(), "ключ"), None);
        assert_eq!(raw_contains_ignore_case(b"plain ascii", "ключ"), None);
        assert_eq!(raw_contains_ignore_case("漢字".as_bytes(), "漢"), None);
    }

    #[test]
    fn a_filtered_page_is_the_first_matches_in_order() {
        let dex = dex::tests::const_string_fixture(4);
        let zip = build_zip(&[("classes.dex", &dex, true)]);
        let path = temp_apk("strings-page", &zip);

        let all = list_strings(&path, 2, false, None, None, None).unwrap();
        let every = |needle: &str| -> Vec<String> {
            let needle = needle.to_lowercase();
            all.iter()
                .filter(|entry| entry.value.to_lowercase().contains(&needle))
                .map(|entry| entry.value.clone())
                .collect()
        };

        for (needle, limit) in [
            ("fixture", 3usize),
            ("m", 2),
            ("AUTHORIZATION", 1),
            ("nothing", 5),
        ] {
            let page = list_strings(&path, 2, false, Some(needle), Some(limit), None).unwrap();
            let expected: Vec<String> = every(needle).into_iter().take(limit).collect();
            assert_eq!(
                page.iter()
                    .map(|entry| entry.value.clone())
                    .collect::<Vec<_>>(),
                expected,
                "{needle:?} limit {limit}"
            );
        }

        for (needle, offset, limit) in [
            ("fixture", 1usize, 2usize),
            ("m", 2, 1),
            ("AUTHORIZATION", 0, 1),
            ("fixture", 99, 5),
        ] {
            let page =
                list_strings(&path, 2, false, Some(needle), Some(limit), Some(offset)).unwrap();
            let expected: Vec<String> =
                every(needle).into_iter().skip(offset).take(limit).collect();
            assert_eq!(
                page.iter()
                    .map(|entry| entry.value.clone())
                    .collect::<Vec<_>>(),
                expected,
                "{needle:?} offset {offset} limit {limit}"
            );
        }

        // A limit with no filter is the head of the table.
        let head = list_strings(&path, 2, false, None, Some(2), None).unwrap();
        assert_eq!(head.len(), 2);
        assert_eq!(head[0].value, all[0].value);
        assert_eq!(head[1].value, all[1].value);

        std::fs::remove_file(&path).unwrap();
    }

    /// An overstated second entry proves a full page stops before inflation.
    #[test]
    fn a_full_page_never_reads_the_entries_below_it() {
        let dex = dex::tests::const_string_fixture(2);
        let mut zip = build_zip(&[("classes.dex", &dex, true), ("classes2.dex", &dex, true)]);
        overstate_second_entry_size(&mut zip);
        let path = temp_apk("strings-stop", &zip);

        let page = list_strings(&path, 1, false, None, Some(1), None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].index, 0);

        // Without the limit the walk has to inflate that entry and cannot.
        let all = list_strings(&path, 1, false, None, None, None);
        assert!(all.is_err(), "the second entry should not inflate: {all:?}");

        std::fs::remove_file(&path).unwrap();
    }

    fn overstate_second_entry_size(zip: &mut [u8]) {
        let mut seen = 0;
        for offset in 0..zip.len().saturating_sub(4) {
            if zip[offset..offset + 4] == [0x50, 0x4b, 0x01, 0x02] {
                seen += 1;
                if seen == 2 {
                    let size =
                        u32::from_le_bytes(zip[offset + 24..offset + 28].try_into().unwrap()) + 1;
                    zip[offset + 24..offset + 28].copy_from_slice(&size.to_le_bytes());
                    return;
                }
            }
        }
        panic!("the fixture's second central-directory entry was not found");
    }

    #[test]
    fn a_bare_dex_is_one_stored_entry() {
        let dex = dex::tests::const_string_fixture(2);
        let entries = parse_dex_entries(dex.as_slice()).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "classes.dex");
        assert_eq!(entry.compression, 0);
        assert_eq!(entry.uncompressed_size, dex.len());
        assert_eq!(
            crate::archive::inflate_entry(dex.as_slice(), entry, usize::MAX).unwrap(),
            dex
        );
    }

    #[test]
    fn a_zip_is_not_read_as_a_bare_dex() {
        let dex = dex::tests::const_string_fixture(1);
        let zip = build_zip(&[("classes.dex", &dex, true)]);
        assert_eq!(parse_dex_entries(zip.as_slice()).unwrap().len(), 1);
        assert!(!is_bare_dex(zip.as_slice()));
        assert!(is_bare_dex(dex.as_slice()));
        // A truncated or unrelated file is not a DEX either, and does not panic.
        assert!(!is_bare_dex(&b"dex\n"[..]));
        assert!(!is_bare_dex(&b"dex\nXX9\0"[..]));
    }

    #[test]
    fn bare_and_wrapped_dex_agree_on_every_command() {
        let dex = dex::tests::const_string_fixture(3);
        let zip = build_zip(&[("classes.dex", &dex, true)]);
        let bare = temp_apk("bare", &dex);
        let wrapped = temp_apk("wrapped", &zip);

        assert_eq!(
            list_classes(&bare, 2, false).unwrap(),
            list_classes(&wrapped, 2, false).unwrap()
        );
        assert_eq!(
            find_references(
                &bare,
                &Query::String(Pattern::search("Authorization", false)),
                2,
                false
            )
            .unwrap(),
            find_references(
                &wrapped,
                &Query::String(Pattern::search("Authorization", false)),
                2,
                false
            )
            .unwrap()
        );
        assert_eq!(
            decompile_class(&bare, "LFixture1;", 2, false).unwrap(),
            decompile_class(&wrapped, "LFixture1;", 2, false).unwrap()
        );

        std::fs::remove_file(&bare).unwrap();
        std::fs::remove_file(&wrapped).unwrap();
    }

    #[test]
    fn duplicate_manifest_entries_use_the_last_copy() {
        let zip = build_zip(&[
            ("AndroidManifest.xml", b"first".as_slice(), false),
            ("AndroidManifest.xml", b"second".as_slice(), false),
        ]);
        let path = temp_apk("duplicate-manifest", &zip);
        assert_eq!(read_entry(&path, "AndroidManifest.xml").unwrap(), b"second");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn archive_policies_are_isolated_across_calls_and_threads() {
        let payload = vec![7u8; 128 * 1024];
        let zip = build_zip(&[("AndroidManifest.xml", payload.as_slice(), true)]);
        let path = temp_apk("policy-isolation", &zip);
        let restrictive = ArchivePolicy {
            max_inflated_entry: Some(1024),
        };
        let permissive = ArchivePolicy {
            max_inflated_entry: None,
        };

        assert!(read_entry_with_policy(&path, restrictive, "AndroidManifest.xml").is_err());
        assert_eq!(
            read_entry_with_policy(&path, permissive, "AndroidManifest.xml").unwrap(),
            payload
        );

        std::thread::scope(|scope| {
            let rejected = scope.spawn(|| {
                read_entry_with_policy(&path, restrictive, "AndroidManifest.xml").is_err()
            });
            let accepted = scope.spawn(|| {
                read_entry_with_policy(&path, permissive, "AndroidManifest.xml").unwrap()
            });
            assert!(rejected.join().unwrap());
            assert_eq!(accepted.join().unwrap(), payload);
        });
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn decompiles_a_class_from_a_synthetic_apk() {
        let dex = dex::tests::const_string_fixture(1);
        let zip = build_zip(&[("classes.dex", &dex, true)]);
        let path = temp_apk("decompile", &zip);
        let (entry, source) = decompile_class(&path, "LFixture0;", 2, false)
            .unwrap()
            .expect("fixture class is found");
        assert_eq!(entry, "classes.dex");
        assert!(source.contains("Fixture0"), "unexpected source: {source}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn named_dex_entries_win_over_the_fallback() {
        let named = dex::tests::const_string_fixture(1);
        let other = dex::tests::const_string_fixture(2);
        let zip = build_zip(&[("app.dex", &other, true), ("classes.dex", &named, true)]);
        let path = temp_apk("prefer-named", &zip);
        let rows = find_references(
            &path,
            &Query::String(Pattern::search("Authorization", false)),
            2,
            false,
        )
        .unwrap();
        assert_eq!(
            rows,
            ["classes.dex | LFixture0;->m0()V | matched=(Authorization)"]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn dex_entries_are_selected_by_name() {
        let zip = build_zip(&[
            ("classes.dex", b"first", false),
            ("classes2.dex", b"second", false),
            ("assets/classes3.dex", b"nested", false),
            ("AndroidManifest.xml", b"<manifest/>", false),
        ]);
        let names: Vec<String> = parse_dex_entries(&zip)
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(
            names,
            ["classes.dex", "classes2.dex"],
            "root DEX entries only"
        );
    }

    #[test]
    fn find_references_searches_every_logical_dex_in_a_041_container() {
        let container = dex::tests::dex041_container(&[2, 1]);
        let zip = build_zip(&[("classes.dex", &container, true)]);
        let path = temp_apk("dex041", &zip);
        let rows = find_references(
            &path,
            &Query::String(Pattern::search("Authorization", false)),
            2,
            false,
        )
        .unwrap();
        assert_eq!(
            rows,
            [
                "classes.dex!classes1.dex | LFixture0;->m0()V | matched=(Authorization)",
                "classes.dex!classes1.dex | LFixture1;->m1()V | matched=(Authorization)",
                "classes.dex!classes2.dex | LFixture0;->m0()V | matched=(Authorization)",
            ]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn decompiles_a_class_from_every_logical_dex_of_a_041_container() {
        // DEX 041 header overlays require a recomputed view checksum.
        // The second member exercises this too.
        let container = dex::tests::dex041_container(&[1, 2]);
        let zip = build_zip(&[("classes.dex", &container, true)]);
        let path = temp_apk("dex041-getclass", &zip);
        for (descriptor, expected_entry) in [
            ("LFixture0;", "classes.dex!classes1.dex"),
            ("LFixture1;", "classes.dex!classes2.dex"),
        ] {
            let hit = decompile_class(&path, descriptor, 2, false).unwrap();
            let (entry, source) = hit.unwrap_or_else(|| panic!("{descriptor} did not decompile"));
            assert!(entry.starts_with(expected_entry), "found in {entry}");
            assert!(source.contains("Fixture"), "unexpected source: {source}");
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_entry_that_is_not_a_dex_is_a_clean_error() {
        let good = dex::tests::const_string_fixture(1);
        let zip = build_zip(&[
            ("classes.dex", b"JUNKJUNKJUNK".as_slice(), false),
            ("classes2.dex", &good, true),
        ]);
        let path = temp_apk("junk-dex", &zip);
        let error = find_references(
            &path,
            &Query::String(Pattern::search("Authorization", false)),
            2,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("invalid DEX header"),
            "unexpected error: {error}"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn find_class_finds_a_class_whose_type_id_is_a_later_duplicate() {
        // Repeated descriptors must resolve through each class_def's own type id.
        let mut dex = dex::tests::const_string_fixture(2);
        let header_size = crate::bytes::read_u32(&dex, 0x24).unwrap() as usize;
        let strings = crate::bytes::read_u32(&dex, 0x38).unwrap() as usize;
        // The first matching type_id has no class_def; the later one does.
        crate::archive::tests::write_u32(&mut dex, header_size + strings * 4, 1);
        let zip = build_zip(&[("classes.dex", &dex, true)]);
        let path = temp_apk("duplicate-type-id", &zip);
        let listed = list_classes(&path, 2, false).unwrap();
        assert!(
            listed.iter().any(|entry| entry.descriptor == "LFixture0;"),
            "fixture stopped listing the class: {listed:?}"
        );
        let hit = map_class_hit(
            &path,
            ArchivePolicy::default(),
            "LFixture0;",
            2,
            false,
            |data| Ok(data.to_vec()),
        )
        .unwrap();
        assert!(
            hit.is_some(),
            "listed by classes but missing from the lookup"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn class_index_reads_041_containers_through_the_full_path() {
        // DEX 041 prefixes require full-reader fallback for every member.
        let container = dex::tests::dex041_container(&[2, 1]);
        let zip = build_zip(&[("classes.dex", &container, true)]);
        let path = temp_apk("dex041-classes", &zip);
        let listed = list_classes(&path, 2, false).unwrap();

        let names: Vec<&str> = listed.iter().map(|entry| &*entry.dex_name).collect();
        assert_eq!(
            names,
            [
                "classes.dex!classes1.dex",
                "classes.dex!classes1.dex",
                "classes.dex!classes2.dex"
            ],
            "{listed:?}"
        );
        // One row per definition: LFixture0; appears in both logical members.
        let descriptors: Vec<&str> = listed
            .iter()
            .map(|entry| entry.descriptor.as_str())
            .collect();
        assert_eq!(
            descriptors,
            ["LFixture0;", "LFixture1;", "LFixture0;"],
            "{listed:?}"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn single_member_041_container_reports_the_plain_dex_name() {
        let container = dex::tests::dex041_container(&[1]);
        let zip = build_zip(&[("classes.dex", &container, true)]);
        let path = temp_apk("dex041-single", &zip);
        let rows = find_references(
            &path,
            &Query::String(Pattern::search("Authorization", false)),
            2,
            false,
        )
        .unwrap();
        assert_eq!(
            rows,
            ["classes.dex | LFixture0;->m0()V | matched=(Authorization)"]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn find_references_runs_end_to_end_on_a_synthetic_apk() {
        let first = dex::tests::const_string_fixture(2);
        let second = dex::tests::const_string_fixture(1);
        let zip = build_zip(&[
            ("classes.dex", &first, true),
            ("classes2.dex", &second, false),
            ("AndroidManifest.xml", b"<manifest/>", false),
        ]);
        let path = temp_apk("synthetic", &zip);
        let query = Query::String(Pattern::search("Authorization", false));
        let rows = find_references(&path, &query, 2, false).unwrap();
        assert_eq!(
            rows,
            [
                "classes.dex | LFixture0;->m0()V | matched=(Authorization)",
                "classes.dex | LFixture1;->m1()V | matched=(Authorization)",
                "classes2.dex | LFixture0;->m0()V | matched=(Authorization)",
            ]
        );
        assert_eq!(
            find_references(&path, &query, 2, false).unwrap(),
            rows,
            "deterministic"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn worker_count_must_be_positive() {
        let zip = build_zip(&[("classes.dex", b"payload", false)]);
        let path = temp_apk("workers", &zip);
        let query = Query::String(Pattern::search("x", false));
        assert!(find_references(&path, &query, 0, false).is_err());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn the_inline_sort_key_orders_like_the_descriptor() {
        let samples = [
            "L;",
            "La;",
            "La;b",
            "Lab;",
            "Lcom/a;",
            "Lcom/ab;",
            "Lcom/a/b;",
            "Lcom/b;",
            "Lcom/exampleprefix/aaa;",
            "Lcom/exampleprefix/aab;",
            "Lcom/exampleprefix/zzz;",
            "Lé;",
            "L中;",
            "L\u{1F600};",
            "Lcom/example/Main$Nested;",
            "Lcom/example/Main$Nested$1;",
        ];
        let entries: Vec<ClassEntry> = samples
            .iter()
            .enumerate()
            .map(|(index, descriptor)| {
                ClassEntry::new(
                    (*descriptor).to_owned(),
                    None,
                    format!("classes{index}.dex").into(),
                )
            })
            .collect();
        for left in &entries {
            for right in &entries {
                let plain = left
                    .descriptor
                    .cmp(&right.descriptor)
                    .then_with(|| left.dex_name.cmp(&right.dex_name));
                assert_eq!(
                    left.cmp(right),
                    plain,
                    "{:?} vs {:?}",
                    left.descriptor,
                    right.descriptor
                );
            }
        }
        // A sorted copy of the samples must match sorting by the plain key.
        let mut sorted = entries.clone();
        sorted.sort_unstable();
        let mut plain: Vec<ClassEntry> = entries;
        plain.sort_by(|left, right| {
            left.descriptor
                .cmp(&right.descriptor)
                .then_with(|| left.dex_name.cmp(&right.dex_name))
        });
        assert_eq!(
            sorted.iter().map(|e| &e.descriptor).collect::<Vec<_>>(),
            plain.iter().map(|e| &e.descriptor).collect::<Vec<_>>()
        );
    }

    /// Manual timing harness: RASC_BENCH_APK=<path> cargo test --release
    /// -- --ignored --nocapture inproc_paths
    #[test]
    #[ignore = "timing harness: run with --release --ignored --nocapture and RASC_BENCH_APK"]
    fn inproc_paths() {
        let Ok(apk) = std::env::var("RASC_BENCH_APK") else {
            println!("[inproc] RASC_BENCH_APK not set, skipping");
            return;
        };
        let path = std::path::Path::new(&apk);
        const CALLS: u32 = 5;

        let _ = read_entry(path, "AndroidManifest.xml").unwrap();
        let started = Instant::now();
        for _ in 0..CALLS {
            let _ = read_entry(path, "AndroidManifest.xml").unwrap();
        }
        println!(
            "[inproc] read_entry(manifest): {:.1} us/call",
            started.elapsed().as_secs_f64() * 1e6 / f64::from(CALLS)
        );

        // The decode half of the manifest command, which is where the vendored AXML
        // parser's per-attribute allocations live: `read_entry` above is only the bytes.
        let manifest_bytes = read_entry(path, "AndroidManifest.xml").unwrap();
        let _ = crate::manifest::decode(&manifest_bytes).unwrap();
        let started = Instant::now();
        for _ in 0..CALLS {
            let _ = crate::manifest::decode(&manifest_bytes).unwrap();
        }
        println!(
            "[inproc] manifest::decode: {:.1} us/call ({} KiB of AXML)",
            started.elapsed().as_secs_f64() * 1e6 / f64::from(CALLS),
            manifest_bytes.len() / 1024
        );

        let classes = list_classes(path, 1, false).unwrap();
        let started = Instant::now();
        for _ in 0..CALLS {
            let _ = list_classes(path, 1, false).unwrap();
        }
        println!(
            "[inproc] list_classes(1 thread): {:.1} us/call ({} classes)",
            started.elapsed().as_secs_f64() * 1e6 / f64::from(CALLS),
            classes.len()
        );

        if let Some(first) = classes.first() {
            let descriptor = first.descriptor.clone();
            let _ = decompile_class(path, &descriptor, 1, false).unwrap();
            let started = Instant::now();
            for _ in 0..CALLS {
                let _ = decompile_class(path, &descriptor, 1, false).unwrap();
            }
            println!(
                "[inproc] decompile_class({descriptor}): {:.1} us/call",
                started.elapsed().as_secs_f64() * 1e6 / f64::from(CALLS)
            );
        }
    }

    #[test]
    fn class_entry_exposes_name_components() {
        let entry = ClassEntry::new(
            "Lcom/example/Main$Nested;".to_owned(),
            None,
            "classes.dex".into(),
        );
        assert_eq!(entry.java_name(), "com/example/Main$Nested");
        assert_eq!(entry.package(), "com/example");
        assert_eq!(entry.simple_name(), "Main$Nested");
    }
}
