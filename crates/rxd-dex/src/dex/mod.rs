//! Borrowed DEX tables and reference scanning with checked bounds.
//! Target operands are little-endian indices at pc + 2.

pub(crate) mod container;
mod filter;
pub(crate) mod methods;
mod mutf8;
mod opcodes;
pub(crate) mod prefix;

use crate::bytes::{read_u16, read_u32};
use crate::query::{MemberQuery, Pattern, Query};
use anyhow::{Context, Result, bail};
use filter::Targets;
use memchr::memmem::Finder;
use rayon::prelude::*;

/// Class lists at least this large are split across workers during the scan.
const PARALLEL_SCAN_CLASSES: usize = 64;
/// Minimum classes per split; keeps splitting overhead negligible on huge DEXes.
const PARALLEL_SCAN_MIN_CHUNK: usize = 8;

#[derive(Clone, Copy, Debug)]
struct Header {
    strings_size: usize,
    strings_off: usize,
    types_size: usize,
    types_off: usize,
    protos_size: usize,
    protos_off: usize,
    fields_size: usize,
    fields_off: usize,
    methods_size: usize,
    methods_off: usize,
    classes_size: usize,
    classes_off: usize,
}

#[derive(Clone, Copy, Debug)]
struct MemberId {
    class_idx: u16,
    name_idx: u32,
}

pub fn defines_class(data: &[u8], descriptor: &[u8]) -> Result<bool> {
    use crate::profile::{Counter, Stage, count, measure};
    count(Counter::DexFiles, 1);
    count(Counter::Bytes, data.len() as u64);
    let dex = measure(Stage::Header, || Dex::parse(data))?;
    count(Counter::Classes, dex.header.classes_size as u64);
    // Resolve each class_def's type id; descriptors may repeat across type_ids.
    for index in 0..dex.header.classes_size {
        let offset = dex.header.classes_off + index * 32;
        let type_idx = dex.u32(offset)? as usize;
        if let Ok(string_idx) = dex.type_string_idx(type_idx)
            && dex.string_bytes(string_idx)? == descriptor
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Visit raw MUTF-8 strings in table order; false stops the scan.
pub fn for_each_string(
    data: &[u8],
    mut visit: impl FnMut(usize, &[u8]) -> Result<bool>,
) -> Result<()> {
    use crate::profile::{Counter, Stage, count, measure};
    count(Counter::DexFiles, 1);
    count(Counter::Bytes, data.len() as u64);
    let dex = measure(Stage::Header, || Dex::parse(data))?;
    count(Counter::Strings, dex.header.strings_size as u64);
    for index in 0..dex.header.strings_size {
        if !visit(index, dex.string_bytes(index)?)? {
            return Ok(());
        }
    }
    Ok(())
}

pub fn decode_string(bytes: &[u8]) -> String {
    mutf8::decode_owned(bytes)
}

pub fn class_names(data: &[u8]) -> Result<Vec<ClassName>> {
    use crate::profile::{Counter, Stage, count, measure};
    count(Counter::DexFiles, 1);
    count(Counter::Bytes, data.len() as u64);
    let names = measure(Stage::Members, || class_definitions(data, 0, usize::MAX))?;
    count(Counter::Classes, names.len() as u64);
    Ok(names
        .into_iter()
        .map(|definition| (definition.descriptor, definition.super_descriptor))
        .collect())
}

/// Class descriptor and optional non-Object superclass.
pub type ClassName = (String, Option<String>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClassDefinition {
    pub(crate) class_def_idx: usize,
    pub(crate) descriptor: String,
    /// None for Object or absent superclass.
    pub(crate) super_descriptor: Option<String>,
}

/// A bounded slice of the class definition table, preserving its exact row identity.
pub(crate) fn class_definitions(
    data: &[u8],
    start: usize,
    limit: usize,
) -> Result<Vec<ClassDefinition>> {
    let mut definitions = Vec::new();
    for_each_class_definition(data, |definition| {
        if definition.class_def_idx >= start && definitions.len() < limit {
            definitions.push(definition);
        }
        Ok(definitions.len() < limit)
    })?;
    Ok(definitions)
}

/// Visit physical class definitions without materializing them; false stops.
pub(crate) fn for_each_class_definition(
    data: &[u8],
    mut visit: impl FnMut(ClassDefinition) -> Result<bool>,
) -> Result<()> {
    let dex = Dex::parse(data)?;
    for index in 0..dex.header.classes_size {
        let class_def = dex.header.classes_off + index * 32;
        let definition = ClassDefinition {
            class_def_idx: index,
            descriptor: dex.type_name(dex.u32(class_def)? as usize)?,
            super_descriptor: super_descriptor(&dex, class_def)?,
        };
        if !visit(definition)? {
            break;
        }
    }
    Ok(())
}

/// Resolve superclass through type_ids; omit Object and missing superclasses.
fn super_descriptor(dex: &Dex<'_>, class_def: usize) -> Result<Option<String>> {
    const NO_INDEX: u32 = u32::MAX;
    let index = dex.u32(class_def + 8)?;
    if index == NO_INDEX {
        return Ok(None);
    }
    // A descriptor is the identity; the row prints the internal name the JVM backend also
    // prints, so the same class looks the same in both backends' output.
    let name = dex.type_name(index as usize)?;
    let internal = name
        .strip_prefix('L')
        .and_then(|name| name.strip_suffix(';'))
        .unwrap_or(name.as_str());
    Ok((internal != "java/lang/Object").then(|| internal.to_owned()))
}

/// Caller/target pairs sorted and deduplicated after scanning.
type Hit = (u32, u32);

/// One caller/target reference pair; CLI owns rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceRow {
    /// Exact DEX method_ids identity of the referencing method.
    pub method_index: u32,
    /// Caller descriptor and method name: Lclass;->name.
    pub member: String,
    /// One exact referenced coordinate accepted by --at.
    pub matched: String,
}

#[derive(Default)]
struct TargetMemo {
    last: Option<(usize, String)>,
}

#[cfg(test)]
pub fn find_references(data: &[u8], query: &Query) -> Result<Vec<ReferenceRow>> {
    Ok(find_references_with_anchor(data, query)?.0)
}

/// Return references plus anchor existence; missing anchors are not zero hits.
pub fn find_references_with_anchor(
    data: &[u8],
    query: &Query,
) -> Result<(Vec<ReferenceRow>, bool)> {
    use crate::profile::{Counter, Stage, count, measure};
    count(Counter::DexFiles, 1);
    count(Counter::Bytes, data.len() as u64);
    let dex = measure(Stage::Header, || Dex::parse(data))?;
    let (kind, targets) = measure(Stage::Members, || dex.resolve_targets(query))?;
    if targets.is_empty() {
        return Ok((Vec::new(), false));
    }
    let targets = Targets::new(targets);
    let hits = measure(Stage::Scan, || dex.scan_all_classes(kind, &targets))?;
    let rows = measure(Stage::Render, || render_reference_rows(&dex, kind, &hits))?;
    count(Counter::Hits, rows.len() as u64);
    Ok((rows, true))
}

#[derive(Debug)]
pub(crate) struct ReferenceScratchLimit;

impl std::fmt::Display for ReferenceScratchLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("reference scan scratch exceeds the configured result-byte budget")
    }
}

impl std::error::Error for ReferenceScratchLimit {}

#[cfg(test)]
pub(crate) fn for_each_reference(
    data: &[u8],
    query: &Query,
    max_scratch_bytes: usize,
    mut visit: impl FnMut(ReferenceRow) -> Result<bool>,
) -> Result<()> {
    let dex = Dex::parse(data)?;
    let (kind, targets) = dex.resolve_targets_bounded(query, max_scratch_bytes)?;
    if targets.is_empty() {
        return Ok(());
    }
    let targets = Targets::new(targets);
    let max_hits = max_scratch_bytes / std::mem::size_of::<Hit>();
    for class_index in 0..dex.header.classes_size {
        let mut hits = Vec::new();
        dex.scan_class_index_bounded(class_index, kind, &targets, &mut hits, max_hits)?;
        if hits.is_empty() {
            continue;
        }
        hits.sort_unstable();
        hits.dedup();
        let mut member_memo: Option<(u32, String)> = None;
        let mut target_memo = TargetMemo::default();
        for &(method_idx, index) in &hits {
            let method = dex.method(method_idx as usize)?;
            let member_bound = dex
                .string_bytes(dex.type_string_idx(method.class_idx as usize)?)?
                .len()
                .saturating_add(2)
                .saturating_add(dex.string_bytes(method.name_idx as usize)?.len());
            if member_bound > max_scratch_bytes {
                return Err(ReferenceScratchLimit.into());
            }
            // Rows are sorted by caller, so the same member name repeats for every target
            // that caller references.
            let member = match &member_memo {
                Some((memoized, member)) if *memoized == method_idx => member.clone(),
                _ => {
                    let member = dex.member_name(method_idx as usize)?;
                    member_memo = Some((method_idx, member.clone()));
                    member
                }
            };
            let mut matched = String::new();
            let bound = member
                .len()
                .saturating_add(dex.target_name_len_upper_bound(kind, index as usize)?);
            if bound > max_scratch_bytes {
                return Err(ReferenceScratchLimit.into());
            }
            dex.push_target_name(kind, index as usize, &mut target_memo, &mut matched)?;
            if !visit(ReferenceRow {
                method_index: method_idx,
                member,
                matched,
            })? {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn render_reference_rows(dex: &Dex<'_>, kind: RefKind, hits: &[Hit]) -> Result<Vec<ReferenceRow>> {
    let mut rows: Vec<ReferenceRow> = Vec::with_capacity(hits.len());
    let mut memo = TargetMemo::default();
    // Memoize caller names across their sorted target hits.
    let mut member_memo: Option<(u32, String)> = None;
    for &(method_idx, index) in hits {
        let mut matched = String::new();
        dex.push_target_name(kind, index as usize, &mut memo, &mut matched)?;
        let member = match &member_memo {
            Some((memoized, member)) if *memoized == method_idx => member.clone(),
            _ => {
                let member = dex.member_name(method_idx as usize)?;
                member_memo = Some((method_idx, member.clone()));
                member
            }
        };
        rows.push(ReferenceRow {
            method_index: method_idx,
            member,
            matched,
        });
    }
    Ok(rows)
}

/// Resolve targets from a complete prefix.
/// None requires full parsing; an empty target set safely skips the entry.
pub fn prefix_has_targets(prefix: &[u8], query: &Query) -> Result<Option<bool>> {
    let Ok(dex) = Dex::parse(prefix) else {
        return Ok(None);
    };
    match dex.resolve_targets(query) {
        Ok((_kind, targets)) => Ok(Some(!targets.is_empty())),
        Err(_error) => Ok(None),
    }
}

/// MUTF-8 substring selector or exact coordinate anchor.
struct Selector<'a> {
    needle: &'a [u8],
    finder: Option<Finder<'a>>,
    /// Case folding requires decoded strings, not raw MUTF-8 bytes.
    folded: Option<String>,
}

impl<'a> Selector<'a> {
    fn new(needle: &'a [u8], exact: bool, case_sensitive: bool) -> Self {
        Self {
            needle,
            finder: (!exact && case_sensitive).then(|| Finder::new(needle)),
            folded: (!case_sensitive).then(|| mutf8::decode_owned(needle).to_lowercase()),
        }
    }

    fn matches(&mut self, value: &[u8]) -> bool {
        if let Some(folded) = &self.folded {
            let decoded = mutf8::decode_owned(value).to_lowercase();
            return if self.finder.is_none() && self.exact() {
                decoded == *folded
            } else {
                decoded.contains(folded.as_str())
            };
        }
        match &self.finder {
            Some(finder) => finder.find(value).is_some(),
            None => value == self.needle,
        }
    }

    fn exact(&self) -> bool {
        self.finder.is_none() && self.folded.is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefKind {
    String,
    Type,
    Method,
    Field,
}

impl RefKind {
    const fn bit(self) -> u8 {
        match self {
            RefKind::String => 1,
            RefKind::Type => 2,
            RefKind::Method => 4,
            RefKind::Field => 8,
        }
    }
}

struct Dex<'a> {
    data: &'a [u8],
    header: Header,
}

impl<'a> Dex<'a> {
    fn parse(data: &'a [u8]) -> Result<Self> {
        if data.len() < 0x70 || !data.starts_with(b"dex\n") {
            bail!("invalid DEX header");
        }
        let header = Header {
            strings_size: read_u32(data, 0x38)? as usize,
            strings_off: read_u32(data, 0x3c)? as usize,
            types_size: read_u32(data, 0x40)? as usize,
            types_off: read_u32(data, 0x44)? as usize,
            protos_size: read_u32(data, 0x48)? as usize,
            protos_off: read_u32(data, 0x4c)? as usize,
            fields_size: read_u32(data, 0x50)? as usize,
            fields_off: read_u32(data, 0x54)? as usize,
            methods_size: read_u32(data, 0x58)? as usize,
            methods_off: read_u32(data, 0x5c)? as usize,
            classes_size: read_u32(data, 0x60)? as usize,
            classes_off: read_u32(data, 0x64)? as usize,
        };
        for (offset, count, width, name) in [
            (header.strings_off, header.strings_size, 4, "string_ids"),
            (header.types_off, header.types_size, 4, "type_ids"),
            (header.protos_off, header.protos_size, 12, "proto_ids"),
            (header.fields_off, header.fields_size, 8, "field_ids"),
            (header.methods_off, header.methods_size, 8, "method_ids"),
            (header.classes_off, header.classes_size, 32, "class_defs"),
        ] {
            check_table(data, offset, count, width, name)?;
        }
        Ok(Self { data, header })
    }

    fn u32(&self, offset: usize) -> Result<u32> {
        read_u32(self.data, offset)
    }

    fn string_bytes(&self, index: usize) -> Result<&'a [u8]> {
        if index >= self.header.strings_size {
            bail!("string index out of range");
        }
        let mut offset = self.u32(self.header.strings_off + index * 4)? as usize;
        if offset >= self.data.len() {
            bail!("string_data_off outside DEX");
        }
        read_uleb(self.data, &mut offset)?;
        let tail = self.data.get(offset..).context("bad string data offset")?;
        let end = memchr::memchr(0, tail).context("unterminated DEX string")?;
        Ok(&tail[..end])
    }

    fn string(&self, index: usize) -> Result<String> {
        let bytes = self.string_bytes(index)?;
        Ok(mutf8::decode_owned(bytes))
    }

    fn type_string_idx(&self, index: usize) -> Result<usize> {
        if index >= self.header.types_size {
            bail!("type index out of range");
        }
        Ok(self.u32(self.header.types_off + index * 4)? as usize)
    }

    /// Include the prototype to distinguish overloaded method coordinates.
    fn member_name(&self, method_idx: usize) -> Result<String> {
        let method = self.method(method_idx)?;
        let class = self.string_bytes(self.type_string_idx(method.class_idx as usize)?)?;
        let name = self.string_bytes(method.name_idx as usize)?;
        let proto_idx = read_u16(self.data, self.header.methods_off + method_idx * 8 + 2)? as usize;
        let mut out = String::with_capacity(class.len() + name.len() + 24);
        mutf8::push_decoded(&mut out, class);
        out.push_str("->");
        mutf8::push_decoded(&mut out, name);
        out.push_str(&self.proto(proto_idx)?);
        Ok(out)
    }

    fn type_name(&self, index: usize) -> Result<String> {
        self.string(self.type_string_idx(index)?)
    }

    /// Resolve the shared u16 prototype/type slot for methods/fields.
    fn member_descriptor(&self, base: usize, index: usize, methods: bool) -> Result<String> {
        let descriptor_idx = read_u16(self.data, base + index * 8 + 2)? as usize;
        if methods {
            self.proto(descriptor_idx)
        } else {
            self.type_name(descriptor_idx)
        }
    }

    /// DEX method prototype, including return type, e.g. (I)V.
    fn proto(&self, index: usize) -> Result<String> {
        if index >= self.header.protos_size {
            bail!("proto index out of range");
        }
        let row = self.header.protos_off + index * 12;
        let return_type_idx = self.u32(row + 4)? as usize;
        let parameters_off = self.u32(row + 8)? as usize;

        let mut out = String::from("(");
        if parameters_off != 0 {
            let size = self.u32(parameters_off)? as usize;
            // `type_list`: a u32 count followed by that many u16 type indices.
            let list = parameters_off + 4;
            check_table(self.data, list, size, 2, "type_list")?;
            for position in 0..size {
                out.push_str(&self.type_name(read_u16(self.data, list + position * 2)? as usize)?);
            }
        }
        out.push(')');
        out.push_str(&self.type_name(return_type_idx)?);
        Ok(out)
    }

    fn method(&self, index: usize) -> Result<MemberId> {
        self.member(self.header.methods_off, self.header.methods_size, index)
    }

    fn field(&self, index: usize) -> Result<MemberId> {
        self.member(self.header.fields_off, self.header.fields_size, index)
    }

    fn member(&self, base: usize, size: usize, index: usize) -> Result<MemberId> {
        if index >= size {
            bail!("member index out of range");
        }
        let offset = base + index * 8;
        Ok(MemberId {
            class_idx: read_u16(self.data, offset)?,
            name_idx: self.u32(offset + 4)?,
        })
    }

    fn matching_strings_bounded(&self, pattern: &Pattern, max_targets: usize) -> Result<Vec<u32>> {
        if pattern.text.is_empty() {
            return Ok(Vec::new());
        }
        let needle = mutf8::encode_mutf8(&pattern.text);
        let mut selector = Selector::new(&needle, pattern.exact, pattern.case_sensitive);
        let mut out = Vec::new();
        for index in 0..self.header.strings_size {
            if selector.matches(self.string_bytes(index)?) {
                if out.len() == max_targets {
                    return Err(ReferenceScratchLimit.into());
                }
                out.push(index as u32);
            }
        }
        Ok(out)
    }

    fn matching_types_bounded(&self, pattern: &Pattern, max_targets: usize) -> Result<Vec<u32>> {
        if pattern.text.is_empty() {
            return Ok(Vec::new());
        }
        let needle = mutf8::encode_mutf8(&pattern.text);
        let mut selector = Selector::new(&needle, pattern.exact, pattern.case_sensitive);
        let mut out = Vec::new();
        for index in 0..self.header.types_size {
            let string_index = self.type_string_idx(index)?;
            if selector.matches(self.string_bytes(string_index)?) {
                if out.len() == max_targets {
                    return Err(ReferenceScratchLimit.into());
                }
                out.push(index as u32);
            }
        }
        Ok(out)
    }

    fn resolve_targets(&self, query: &Query) -> Result<(RefKind, Vec<u32>)> {
        self.resolve_targets_bounded(query, usize::MAX)
    }

    fn resolve_targets_bounded(
        &self,
        query: &Query,
        max_scratch_bytes: usize,
    ) -> Result<(RefKind, Vec<u32>)> {
        if max_scratch_bytes < Targets::FIXED_BYTES {
            return Err(ReferenceScratchLimit.into());
        }
        let max_targets =
            max_scratch_bytes.saturating_sub(Targets::FIXED_BYTES) / std::mem::size_of::<u32>();
        let (kind, targets) = match query {
            Query::String(pattern) => (
                RefKind::String,
                self.matching_strings_bounded(pattern, max_targets)?,
            ),
            Query::Type(pattern) => (
                RefKind::Type,
                self.matching_types_bounded(pattern, max_targets)?,
            ),
            Query::Method(query) => (
                RefKind::Method,
                self.matching_members_bounded(query, true, max_targets)?,
            ),
            Query::Field(query) => (
                RefKind::Field,
                self.matching_members_bounded(query, false, max_targets)?,
            ),
        };
        if Targets::FIXED_BYTES
            .saturating_add(targets.len().saturating_mul(std::mem::size_of::<u32>()))
            > max_scratch_bytes
        {
            return Err(ReferenceScratchLimit.into());
        }
        Ok((kind, targets))
    }

    fn matching_members_bounded(
        &self,
        query: &MemberQuery,
        methods: bool,
        max_targets: usize,
    ) -> Result<Vec<u32>> {
        let (base, size) = if methods {
            (self.header.methods_off, self.header.methods_size)
        } else {
            (self.header.fields_off, self.header.fields_size)
        };
        // The needles outlive the selectors that borrow them.
        let name_needle = query.name.as_ref().map(|pattern| {
            (
                mutf8::encode_mutf8(&pattern.text),
                pattern.exact,
                pattern.case_sensitive,
            )
        });
        let class_needle = query.class.as_ref().map(|pattern| {
            (
                mutf8::encode_mutf8(&pattern.text),
                pattern.exact,
                pattern.case_sensitive,
            )
        });
        let mut name_selector = name_needle
            .as_ref()
            .map(|(needle, exact, case)| Selector::new(needle, *exact, *case));
        let mut class_selector = class_needle
            .as_ref()
            .map(|(needle, exact, case)| Selector::new(needle, *exact, *case));
        let mut out = Vec::new();
        for index in 0..size {
            let member = self.member(base, size, index)?;
            let name_matches = match &mut name_selector {
                Some(selector) => selector.matches(self.string_bytes(member.name_idx as usize)?),
                None => true,
            };
            if !name_matches {
                continue;
            }
            let class_name_idx = self.type_string_idx(member.class_idx as usize)?;
            let class_bytes = self.string_bytes(class_name_idx)?;
            let class_matches = match &mut class_selector {
                Some(selector) => selector.matches(class_bytes),
                None => true,
            };
            if !class_matches {
                continue;
            }
            if let Some(descriptor) = &query.descriptor
                && self.member_descriptor(base, index, methods)? != *descriptor
            {
                continue;
            }
            if out.len() == max_targets {
                return Err(ReferenceScratchLimit.into());
            }
            out.push(index as u32);
        }
        Ok(out)
    }

    #[cfg(test)]
    fn target_name_len_upper_bound(&self, kind: RefKind, index: usize) -> Result<usize> {
        match kind {
            RefKind::String => Ok(self.string_bytes(index)?.len()),
            RefKind::Type => Ok(self.string_bytes(self.type_string_idx(index)?)?.len()),
            RefKind::Method | RefKind::Field => {
                let member = if kind == RefKind::Method {
                    self.method(index)?
                } else {
                    self.field(index)?
                };
                Ok(self
                    .string_bytes(self.type_string_idx(member.class_idx as usize)?)?
                    .len()
                    .saturating_add(2)
                    .saturating_add(self.string_bytes(member.name_idx as usize)?.len()))
            }
        }
    }

    fn push_target_name(
        &self,
        kind: RefKind,
        index: usize,
        memo: &mut TargetMemo,
        out: &mut String,
    ) -> Result<()> {
        // Memoize exact target descriptors across adjacent reference hits.
        if let Some((memoized, name)) = &memo.last
            && *memoized == index
        {
            out.push_str(name);
            return Ok(());
        }
        let mut name = String::new();
        self.push_target_name_uncached(kind, index, &mut name)?;
        out.push_str(&name);
        memo.last = Some((index, name));
        Ok(())
    }

    fn push_target_name_uncached(
        &self,
        kind: RefKind,
        index: usize,
        out: &mut String,
    ) -> Result<()> {
        match kind {
            RefKind::String => {
                mutf8::push_decoded(out, self.string_bytes(index)?);
                Ok(())
            }
            RefKind::Type => {
                mutf8::push_decoded(out, self.string_bytes(self.type_string_idx(index)?)?);
                Ok(())
            }
            RefKind::Method | RefKind::Field => {
                let member = if kind == RefKind::Method {
                    self.method(index)?
                } else {
                    self.field(index)?
                };

                let class = self.string_bytes(self.type_string_idx(member.class_idx as usize)?)?;
                let name = self.string_bytes(member.name_idx as usize)?;
                mutf8::push_decoded(out, class);
                out.push_str("->");
                mutf8::push_decoded(out, name);
                let descriptor_idx = read_u16(
                    self.data,
                    if kind == RefKind::Method {
                        self.header.methods_off
                    } else {
                        self.header.fields_off
                    } + index * 8
                        + 2,
                )? as usize;
                if kind == RefKind::Method {
                    out.push_str(&self.proto(descriptor_idx)?);
                } else {
                    out.push(':');
                    out.push_str(&self.type_name(descriptor_idx)?);
                }
                Ok(())
            }
        }
    }

    /// Split large class scans across workers; union hits deterministically.
    fn scan_all_classes(&self, kind: RefKind, targets: &Targets) -> Result<Vec<Hit>> {
        // Small class lists are cheaper to scan on the current thread; larger lists are split
        // to shorten tail latency. Both paths are sorted and deduplicated below.
        let mut hits = if self.header.classes_size < PARALLEL_SCAN_CLASSES {
            self.scan_all_classes_sequential(kind, targets)?
        } else {
            self.scan_all_classes_parallel(kind, targets)?
        };
        // Sort/dedup pairs to group targets by method index.
        hits.sort_unstable();
        hits.dedup();
        Ok(hits)
    }

    fn scan_all_classes_sequential(&self, kind: RefKind, targets: &Targets) -> Result<Vec<Hit>> {
        let mut hits = Vec::new();
        for class_index in 0..self.header.classes_size {
            crate::profile::count(crate::profile::Counter::Classes, 1);
            self.scan_class_index(class_index, kind, targets, &mut hits)?;
        }
        Ok(hits)
    }

    fn scan_all_classes_parallel(&self, kind: RefKind, targets: &Targets) -> Result<Vec<Hit>> {
        (0..self.header.classes_size)
            .into_par_iter()
            .with_min_len(PARALLEL_SCAN_MIN_CHUNK)
            .try_fold(Vec::new, |mut hits, class_index| {
                crate::profile::count(crate::profile::Counter::Classes, 1);
                self.scan_class_index(class_index, kind, targets, &mut hits)?;
                Ok(hits)
            })
            .try_reduce(Vec::new, |mut left, mut right| {
                left.append(&mut right);
                Ok(left)
            })
    }

    fn scan_class_index(
        &self,
        class_index: usize,
        kind: RefKind,
        targets: &Targets,
        hits: &mut Vec<Hit>,
    ) -> Result<()> {
        self.scan_class_index_bounded(class_index, kind, targets, hits, usize::MAX)
    }

    fn scan_class_index_bounded(
        &self,
        class_index: usize,
        kind: RefKind,
        targets: &Targets,
        hits: &mut Vec<Hit>,
        max_hits: usize,
    ) -> Result<()> {
        let class_def = self.header.classes_off + class_index * 32;
        let class_data_off = self.u32(class_def + 24)? as usize;
        if class_data_off != 0 {
            self.scan_class_data(class_data_off, kind, targets, hits, max_hits)?;
        }
        Ok(())
    }

    fn scan_class_data(
        &self,
        offset: usize,
        kind: RefKind,
        targets: &Targets,
        hits: &mut Vec<Hit>,
        max_hits: usize,
    ) -> Result<()> {
        let mut cursor = offset;
        let static_fields = read_uleb(self.data, &mut cursor)? as usize;
        let instance_fields = read_uleb(self.data, &mut cursor)? as usize;
        let direct_methods = read_uleb(self.data, &mut cursor)? as usize;
        let virtual_methods = read_uleb(self.data, &mut cursor)? as usize;
        for _ in 0..static_fields + instance_fields {
            read_uleb(self.data, &mut cursor)?;
            read_uleb(self.data, &mut cursor)?;
        }
        for count in [direct_methods, virtual_methods] {
            let mut method_idx = 0u32;
            for _ in 0..count {
                method_idx = method_idx
                    .checked_add(read_uleb(self.data, &mut cursor)?)
                    .context("method index overflow")?;
                read_uleb(self.data, &mut cursor)?;
                let code_off = read_uleb(self.data, &mut cursor)? as usize;
                if code_off != 0 {
                    crate::profile::count(crate::profile::Counter::CodeItems, 1);
                    self.scan_code(method_idx, code_off, kind, targets, hits, max_hits)?;
                }
            }
        }
        Ok(())
    }

    fn scan_code(
        &self,
        method_idx: u32,
        code_off: usize,
        kind: RefKind,
        targets: &Targets,
        hits: &mut Vec<Hit>,
        max_hits: usize,
    ) -> Result<()> {
        let insns_size = self.u32(code_off + 12)? as usize;
        let start = code_off.checked_add(16).context("code offset overflow")?;
        let end = start
            .checked_add(insns_size.checked_mul(2).context("code size overflow")?)
            .context("code range overflow")?;
        let code = self.data.get(start..end).context("code item outside DEX")?;
        if !targets.might_reference(code) {
            return Ok(());
        }
        let kind_bit = kind.bit();
        let mut pc = 0usize;
        while pc + 2 <= code.len() {
            let opcode = code[pc];

            let info = opcodes::OPCODE_INFO[opcode as usize];
            let units = (info & 0xff) as usize;
            let units = if units == 0 {
                opcodes::instruction_units(code, pc)?
            } else {
                units
            };
            // instruction_units is nonzero; only the end bound remains to check.
            if pc + units * 2 > code.len() {
                bail!("invalid instruction width at code offset {}", start + pc);
            }
            // 0x1b is the only reference instruction with a 32-bit index, and
            // only the string mask can reach it.
            if info & u16::from(kind_bit) << 8 != 0 {
                let index = if opcode == 0x1b {
                    read_u32(code, pc + 2)?
                } else {
                    read_u16(code, pc + 2)? as u32
                };
                // Method hits are adjacent; discard duplicate pairs before sorting.
                if targets.contains(index) && hits.last() != Some(&(method_idx, index)) {
                    if hits.len() == max_hits {
                        return Err(ReferenceScratchLimit.into());
                    }
                    hits.push((method_idx, index));
                }
            }
            pc += units * 2;
        }
        Ok(())
    }
}

fn check_table(data: &[u8], offset: usize, count: usize, width: usize, name: &str) -> Result<()> {
    let end = offset
        .checked_add(count.checked_mul(width).context("table size overflow")?)
        .context("table range overflow")?;
    if end > data.len() {
        bail!("bad {name} range");
    }
    Ok(())
}

fn read_uleb(data: &[u8], offset: &mut usize) -> Result<u32> {
    let mut result = 0u32;
    for shift in [0, 7, 14, 21, 28] {
        let byte = *data.get(*offset).context("truncated ULEB128")?;
        *offset += 1;
        result |= u32::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(result);
        }
    }
    bail!("ULEB128 exceeds 5 bytes")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn push_uleb(out: &mut Vec<u8>, mut value: u32) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return;
            }
        }
    }

    /// Build DEX 041 members with container-relative sections and header offsets.
    pub(crate) fn dex041_container(class_counts: &[usize]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut headers = Vec::new();
        for &count in class_counts {
            let offset = out.len();
            out.extend_from_slice(&const_string_fixture_041(count, offset as u32));
            out[offset..offset + 8].copy_from_slice(b"dex\n041\0");
            write_u32(&mut out, offset + 0x74, offset as u32);
            headers.push(offset);
        }
        let total = out.len() as u32;
        for header in headers {
            // container_size covers all members; file_size advances to the next header.
            write_u32(&mut out, header + 0x70, total);
        }
        out
    }

    fn adler32(bytes: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for byte in bytes {
            a = (a + u32::from(*byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    pub(crate) fn const_string_fixture(class_count: usize) -> Vec<u8> {
        const_string_fixture_with(class_count, 0, 0x70)
    }

    /// `const_string_fixture` with a 0x78-byte DEX 041 header, its section offsets
    /// shifted by `base` so the fixture can sit at that container offset.
    fn const_string_fixture_041(class_count: usize, base: u32) -> Vec<u8> {
        const_string_fixture_with(class_count, base, 0x78)
    }

    fn const_string_fixture_with(class_count: usize, base: u32, header_size: usize) -> Vec<u8> {
        assert!(class_count > 0);
        let mut strings = vec!["Authorization".to_owned()];
        for index in 0..class_count {
            strings.push(format!("LFixture{index};"));
        }
        for index in 0..class_count {
            strings.push(format!("m{index}"));
        }
        // "V" (void) backs the single method prototype; without a proto_ids
        // section the decompiler rejects the fixture before decompiling.
        strings.insert(class_count + 1, "V".to_owned());
        let n_strings = strings.len();
        let n_types = class_count + 2;

        let string_ids_off = header_size;
        let type_ids_off = string_ids_off + n_strings * 4;
        let proto_ids_off = type_ids_off + n_types * 4;
        let method_ids_off = proto_ids_off + 12;
        let class_defs_off = method_ids_off + class_count * 8;
        let data_off = class_defs_off + class_count * 32;

        let mut data = Vec::new();
        let mut string_offsets = Vec::with_capacity(n_strings);
        for value in &strings {
            string_offsets.push(data_off + data.len());
            push_uleb(&mut data, value.len() as u32);
            data.extend_from_slice(value.as_bytes());
            data.push(0);
        }

        let mut class_data_offsets = Vec::with_capacity(class_count);
        for index in 0..class_count {
            while !(data_off + data.len()).is_multiple_of(4) {
                data.push(0);
            }
            let code_off = ((data_off + data.len()) as u32) + base;
            data.extend_from_slice(&1_u16.to_le_bytes()); // registers_size: v0
            data.extend_from_slice(&0_u16.to_le_bytes()); // ins_size
            data.extend_from_slice(&0_u16.to_le_bytes()); // outs_size
            data.extend_from_slice(&0_u16.to_le_bytes()); // tries_size
            data.extend_from_slice(&0_u32.to_le_bytes()); // Two loads in one method distinguish instruction uses from referencing methods.
            data.extend_from_slice(&5_u32.to_le_bytes()); // insns_size in code units
            data.extend_from_slice(&0x001a_u16.to_le_bytes()); // const-string v0, #0
            data.extend_from_slice(&0_u16.to_le_bytes());
            data.extend_from_slice(&0x001a_u16.to_le_bytes()); // const-string v0, #0
            data.extend_from_slice(&0_u16.to_le_bytes());
            data.extend_from_slice(&0x000e_u16.to_le_bytes()); // return-void

            class_data_offsets.push(data_off + data.len());
            data.push(0); // static_fields_size
            data.push(0); // instance_fields_size
            data.push(1); // direct_methods_size
            data.push(0); // virtual_methods_size
            push_uleb(&mut data, index as u32); // method_idx_diff
            push_uleb(&mut data, 0); // access_flags
            push_uleb(&mut data, code_off);
        }

        let total = data_off + data.len();
        let mut out = vec![0_u8; total];
        out[..8].copy_from_slice(b"dex\n039\0");
        write_u32(&mut out, 0x20, total as u32);
        write_u32(&mut out, 0x24, header_size as u32);
        write_u32(&mut out, 0x28, 0x1234_5678);
        write_u32(&mut out, 0x38, n_strings as u32);
        write_u32(&mut out, 0x3c, (string_ids_off as u32) + base);
        write_u32(&mut out, 0x40, n_types as u32);
        write_u32(&mut out, 0x44, (type_ids_off as u32) + base);
        write_u32(&mut out, 0x48, 1); // proto_ids_size
        write_u32(&mut out, 0x4c, (proto_ids_off as u32) + base);
        write_u32(&mut out, 0x58, class_count as u32);
        write_u32(&mut out, 0x5c, (method_ids_off as u32) + base);
        write_u32(&mut out, 0x60, class_count as u32);
        write_u32(&mut out, 0x64, (class_defs_off as u32) + base);

        for (index, offset) in string_offsets.iter().enumerate() {
            write_u32(
                &mut out,
                string_ids_off + index * 4,
                (*offset as u32) + base,
            );
        }
        write_u32(&mut out, type_ids_off, 0);
        // type_ids[class_count + 1] -> "V", the prototype's return type.
        write_u32(
            &mut out,
            type_ids_off + (class_count + 1) * 4,
            (1 + class_count) as u32,
        );
        // One prototype: shorty "V", returns void, takes no parameters.
        write_u32(&mut out, proto_ids_off, (1 + class_count) as u32);
        write_u32(&mut out, proto_ids_off + 4, (class_count + 1) as u32);
        write_u32(&mut out, proto_ids_off + 8, 0);
        for (index, class_data_off) in class_data_offsets.iter().enumerate() {
            write_u32(&mut out, type_ids_off + (index + 1) * 4, (index + 1) as u32);

            let method = method_ids_off + index * 8;
            write_u16(&mut out, method, (index + 1) as u16);
            write_u16(&mut out, method + 2, 0);
            write_u32(&mut out, method + 4, (2 + class_count + index) as u32);

            let class_def = class_defs_off + index * 32;
            write_u32(&mut out, class_def, (index + 1) as u32);
            write_u32(&mut out, class_def + 8, u32::MAX);
            write_u32(&mut out, class_def + 16, u32::MAX);
            write_u32(&mut out, class_def + 24, (*class_data_off as u32) + base);
        }
        out[data_off..].copy_from_slice(&data);
        // The decompiler validates Adler-32 over bytes after the 12-byte prefix.
        let checksum = adler32(&out[12..]);
        write_u32(&mut out, 0x08, checksum);
        out
    }

    /// Build an eight-register invoke-static/range to test complete argument decoding.
    #[allow(dead_code)]
    pub(crate) fn range_invoke_fixture() -> Vec<u8> {
        let header_size = 0x70usize;
        let strings = ["LFixture0;", "V", "VIIIIIIII", "call", "target", "I"];
        let string_ids_off = header_size;
        let type_ids_off = string_ids_off + strings.len() * 4;
        let proto_ids_off = type_ids_off + 3 * 4;
        let method_ids_off = proto_ids_off + 2 * 12;
        let class_defs_off = method_ids_off + 2 * 8;
        let data_off = class_defs_off + 32;

        let mut data = Vec::new();
        let mut string_offsets = Vec::with_capacity(strings.len());
        for value in strings {
            string_offsets.push(data_off + data.len());
            push_uleb(&mut data, value.len() as u32);
            data.extend_from_slice(value.as_bytes());
            data.push(0);
        }
        // The eight `I` parameters of `target`'s prototype, 4-byte aligned.
        while !(data_off + data.len()).is_multiple_of(4) {
            data.push(0);
        }
        let type_list_off = data_off + data.len();
        data.extend_from_slice(&8_u32.to_le_bytes());
        for _ in 0..8 {
            data.extend_from_slice(&2_u16.to_le_bytes());
        }
        // `call`'s code: eight `const/4` definitions, one range invoke, return.
        while !(data_off + data.len()).is_multiple_of(4) {
            data.push(0);
        }
        let code_off = data_off + data.len();
        data.extend_from_slice(&9_u16.to_le_bytes()); // registers_size (v0..v8)
        data.extend_from_slice(&0_u16.to_le_bytes()); // ins_size
        data.extend_from_slice(&8_u16.to_le_bytes()); // outs_size
        data.extend_from_slice(&0_u16.to_le_bytes()); // tries_size
        data.extend_from_slice(&0_u32.to_le_bytes()); // debug_info_off
        data.extend_from_slice(&12_u32.to_le_bytes()); // insns_size
        for register in 1..=8_u16 {
            // const/4 vN, #(N-1): -8..=7 is the nibble's range.
            let unit = 0x12 | (register << 8) | ((register - 1) << 12);
            data.extend_from_slice(&unit.to_le_bytes());
        }
        data.extend_from_slice(&0x0877_u16.to_le_bytes()); // invoke-static/range, AA=8
        data.extend_from_slice(&1_u16.to_le_bytes()); // method@1 = target
        data.extend_from_slice(&1_u16.to_le_bytes()); // start register v1
        data.extend_from_slice(&0x000e_u16.to_le_bytes()); // return-void

        // class_data: one direct static method, `call`.
        let class_data_off = data_off + data.len();
        data.push(0); // static_fields_size
        data.push(0); // instance_fields_size
        data.push(1); // direct_methods_size
        data.push(0); // virtual_methods_size
        push_uleb(&mut data, 0); // method_idx_diff -> method 0 (`call`)
        push_uleb(&mut data, 0x9); // ACC_PUBLIC | ACC_STATIC
        push_uleb(&mut data, code_off as u32);

        let total = data_off + data.len();
        let mut out = vec![0_u8; total];
        out[..8].copy_from_slice(b"dex\n039\0");
        write_u32(&mut out, 0x20, total as u32);
        write_u32(&mut out, 0x24, header_size as u32);
        write_u32(&mut out, 0x28, 0x1234_5678);
        write_u32(&mut out, 0x38, strings.len() as u32);
        write_u32(&mut out, 0x3c, string_ids_off as u32);
        write_u32(&mut out, 0x40, 3); // type_ids_size
        write_u32(&mut out, 0x44, type_ids_off as u32);
        write_u32(&mut out, 0x48, 2); // proto_ids_size
        write_u32(&mut out, 0x4c, proto_ids_off as u32);
        write_u32(&mut out, 0x58, 2); // method_ids_size
        write_u32(&mut out, 0x5c, method_ids_off as u32);
        write_u32(&mut out, 0x60, 1); // class_defs_size
        write_u32(&mut out, 0x64, class_defs_off as u32);

        for (index, offset) in string_offsets.iter().enumerate() {
            write_u32(&mut out, string_ids_off + index * 4, *offset as u32);
        }
        // type_ids: the class, void, int.
        write_u32(&mut out, type_ids_off, 0);
        write_u32(&mut out, type_ids_off + 4, 1);
        write_u32(&mut out, type_ids_off + 8, 5);
        // protos: ()V and (IIIIIIII)V.
        write_u32(&mut out, proto_ids_off, 1);
        write_u32(&mut out, proto_ids_off + 4, 1);
        write_u32(&mut out, proto_ids_off + 8, 0);
        write_u32(&mut out, proto_ids_off + 12, 2);
        write_u32(&mut out, proto_ids_off + 16, 1);
        write_u32(&mut out, proto_ids_off + 20, type_list_off as u32);
        // methods: Fixture0.call()V, Fixture0.target(IIIIIIII)V.
        write_u16(&mut out, method_ids_off, 0);
        write_u16(&mut out, method_ids_off + 2, 0);
        write_u32(&mut out, method_ids_off + 4, 3);
        write_u16(&mut out, method_ids_off + 8, 0);
        write_u16(&mut out, method_ids_off + 10, 1);
        write_u32(&mut out, method_ids_off + 12, 4);
        // One public class, no superclass row, code in `class_data`.
        write_u32(&mut out, class_defs_off, 0);
        write_u32(&mut out, class_defs_off + 4, 1);
        write_u32(&mut out, class_defs_off + 8, u32::MAX);
        write_u32(&mut out, class_defs_off + 16, u32::MAX);
        write_u32(&mut out, class_defs_off + 24, class_data_off as u32);

        out[data_off..].copy_from_slice(&data);
        let checksum = adler32(&out[12..]);
        write_u32(&mut out, 0x08, checksum);
        out
    }

    /// Reference rows use ascending method/target order; APK aggregation follows.
    fn expected_rows(class_count: usize) -> Vec<ReferenceRow> {
        (0..class_count)
            .map(|index| ReferenceRow {
                method_index: index as u32,
                member: format!("LFixture{index};->m{index}()V"),
                matched: "Authorization".to_owned(),
            })
            .collect()
    }

    /// Manual scanner timings: cargo test --release -- --ignored --nocapture inproc.
    #[test]
    #[ignore = "timing harness: run with --release --ignored --nocapture"]
    fn inproc_scan_timings() {
        let data = const_string_fixture(2000);

        let queries = [
            (
                "string",
                Query::String(Pattern::search("Authorization", false)),
            ),
            ("type", Query::Type(Pattern::search("Fixture", false))),
            (
                "method",
                Query::Method(crate::query::MemberQuery {
                    name: Some(Pattern::search("m0", false)),
                    class: None,
                    descriptor: None,
                }),
            ),
        ];
        let mut rows = 0usize;
        for (label, query) in &queries {
            // Warm the caches and the allocator. The first round pays for both, so the
            // comparable figure is the last one.
            for _ in 0..5 {
                rows += find_references(&data, query).unwrap().len();
            }
            let mut last = 0.0;
            for _ in 0..5 {
                let started = std::time::Instant::now();
                const CALLS: u32 = 20;
                for _ in 0..CALLS {
                    rows += find_references(&data, query).unwrap().len();
                }
                last = started.elapsed().as_secs_f64() * 1e6 / f64::from(CALLS);
            }
            println!("[inproc] find_references {label}: {last:.1} us/call (rows seen {rows})");
        }
    }

    #[test]
    fn streaming_reference_scan_bounds_per_class_scratch() {
        let data = const_string_fixture(2);
        let error = for_each_reference(
            &data,
            &Query::String(Pattern::search("Authorization", false)),
            0,
            |_| Ok(true),
        )
        .unwrap_err();
        assert!(error.downcast_ref::<ReferenceScratchLimit>().is_some());
    }

    #[test]
    fn parallel_and_sequential_scans_produce_identical_hits() {
        let data = const_string_fixture(PARALLEL_SCAN_CLASSES);
        let dex = Dex::parse(&data).unwrap();
        let (kind, targets) = dex
            .resolve_targets(&Query::String(Pattern::search("Authorization", false)))
            .unwrap();
        let targets = Targets::new(targets);
        let sequential = dex.scan_all_classes_sequential(kind, &targets).unwrap();
        let parallel = dex.scan_all_classes_parallel(kind, &targets).unwrap();
        assert_eq!(sequential.len(), PARALLEL_SCAN_CLASSES);
        assert_eq!(sequential, parallel);
    }

    #[test]
    fn multi_chunk_parallel_scan_keeps_every_hit() {
        let class_count = PARALLEL_SCAN_CLASSES * 3 + 1;
        let data = const_string_fixture(class_count);
        let rows = find_references(
            &data,
            &Query::String(Pattern::search("Authorization", false)),
        )
        .unwrap();
        assert_eq!(rows, expected_rows(class_count));
    }

    fn minimal_class_dex(descriptor: &[u8]) -> Vec<u8> {
        let string_ids_off = 0x70;
        let type_ids_off = 0x74;
        let class_defs_off = 0x78;
        let string_data_off = 0x98;
        let mut data = vec![0; string_data_off + descriptor.len() + 2];
        data[..8].copy_from_slice(b"dex\n039\0");
        data[0x38..0x3c].copy_from_slice(&1_u32.to_le_bytes());
        data[0x3c..0x40].copy_from_slice(&(string_ids_off as u32).to_le_bytes());
        data[0x40..0x44].copy_from_slice(&1_u32.to_le_bytes());
        data[0x44..0x48].copy_from_slice(&(type_ids_off as u32).to_le_bytes());
        data[0x60..0x64].copy_from_slice(&1_u32.to_le_bytes());
        data[0x64..0x68].copy_from_slice(&(class_defs_off as u32).to_le_bytes());
        data[string_ids_off..string_ids_off + 4]
            .copy_from_slice(&(string_data_off as u32).to_le_bytes());
        data[string_data_off] = descriptor.len() as u8;
        data[string_data_off + 1..string_data_off + 1 + descriptor.len()]
            .copy_from_slice(descriptor);
        data
    }

    #[test]
    fn lists_defined_classes_from_class_defs() {
        let data = minimal_class_dex(b"Lcom/example/Main;");
        let names: Vec<_> = class_names(&data)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["Lcom/example/Main;"]);
        assert!(defines_class(&data, b"Lcom/example/Main;").unwrap());
    }

    #[test]
    fn fuzzy_patterns_are_literal_substrings() {
        assert!(Finder::new(b".b[").find(b"a.b[c]").is_some());
        assert!(Finder::new(b"a.c").find(b"abc").is_none());
    }
}
