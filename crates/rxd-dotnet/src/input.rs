use std::{collections::BTreeMap, fs::OpenOptions, io::Read, path::Path, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use cildec::{
    Metadata, PeImage, TableId,
    tables::ColumnKind,
    token::{BlobIndex, GuidIndex, StringIndex},
};

use crate::{CallSignature, Field, Limits, Method, MethodBody, StringRecord, TypeInfo};

pub struct Input<'a> {
    pub source_id: String,
    pub file_bytes: usize,
    pub types: Vec<TypeInfo>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub properties: Vec<crate::Property>,
    pub events: Vec<crate::Event>,
    pub(crate) bytes: &'a [u8],
    pub(crate) image: PeImage<'a>,
    pub(crate) metadata: Metadata<'a>,
    pub(crate) limits: Limits,
    pub(crate) rows: [u32; 64],
    pub(crate) metadata_work: usize,
    pub(crate) type_kinds: BTreeMap<u32, bool>,
    pub(crate) relationships_loaded: bool,
    /// Metadata rows skipped because their own contents were invalid.
    pub unreadable: Vec<crate::Unreadable>,
    /// Full names for referenced base/interface/type tokens.
    pub(crate) type_names: BTreeMap<u32, Arc<String>>,
}

/// Read one untrusted regular file into a buffer the caller keeps alive.
pub fn read_file(path: &Path, limits: Limits) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file(),
        "INVALID_INPUT: input must be a regular file"
    );
    ensure!(
        meta.len() <= limits.input_bytes as u64,
        "RESOURCE_LIMIT: input bytes"
    );
    let mut bytes = Vec::new();
    file.by_ref()
        .take((limits.input_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limits.input_bytes,
        "RESOURCE_LIMIT: input bytes"
    );
    Ok(bytes)
}

impl<'a> Input<'a> {
    /// Parse one buffer. Rows and operands are decoded when a query reads them, so
    /// unrelated corruption stays out of scope; `verify_module` checks the whole file.
    pub fn parse(bytes: &'a [u8], limits: Limits) -> Result<Self> {
        let file_bytes = bytes.len();
        let source_id = rxd_core::identity::input_id(bytes);
        let image = PeImage::parse(bytes).context("INVALID_INPUT: PE parsing")?;
        let metadata = image
            .metadata()
            .context("INVALID_INPUT: metadata structure and references")?;
        let mut rows = [0; 64];
        let mut total = 0usize;
        let tables = metadata.tables();
        for raw in 0..rows.len() as u8 {
            let Some(table) = TableId::from_u8(raw) else {
                continue;
            };
            let count = tables.row_count(table);
            rows[raw as usize] = count;
            total = total
                .checked_add(count as usize)
                .context("RESOURCE_LIMIT: row count")?;
        }
        ensure!(
            limits.work.is_none_or(|limit| total <= limit),
            "RESOURCE_LIMIT: metadata work"
        );
        let mut input = Self {
            source_id,
            file_bytes,
            types: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
            events: Vec::new(),
            bytes,
            image,
            metadata,
            limits,
            rows,
            metadata_work: 0,
            type_kinds: BTreeMap::new(),
            relationships_loaded: false,
            unreadable: Vec::new(),
            type_names: BTreeMap::new(),
        };
        crate::metadata::load(&mut input)?;
        Ok(input)
    }

    /// Opt-in whole-module check: every row, heap index and coded index in the file.
    pub fn verify_module(&self) -> Result<()> {
        ensure!(
            self.image.diagnostics().is_empty() && self.metadata.diagnostics().is_empty(),
            "INVALID_INPUT: PE or metadata deviation"
        );
        let tables = self.metadata.tables();
        for raw in 0..self.rows.len() as u8 {
            let Some(table) = TableId::from_u8(raw) else {
                continue;
            };
            let columns = cildec::tables::columns_of(table);
            for rid in 1..=tables.row_count(table) {
                for (index, kind) in columns.iter().enumerate() {
                    let value = tables
                        .raw_column(table, rid, index)
                        .context("INVALID_INPUT: metadata column")?;
                    match *kind {
                        ColumnKind::U16 | ColumnKind::U32 => {}
                        ColumnKind::String => {
                            self.metadata
                                .strings()
                                .str(StringIndex(value))
                                .context("INVALID_INPUT: string heap index")?;
                        }
                        ColumnKind::Blob => {
                            if value != 0 {
                                self.metadata
                                    .blobs()
                                    .get(BlobIndex(value))
                                    .context("INVALID_INPUT: blob heap index")?;
                            }
                        }
                        ColumnKind::Guid => {
                            if value != 0 {
                                self.metadata
                                    .guids()
                                    .get(GuidIndex(value))
                                    .context("INVALID_INPUT: guid heap index")?;
                            }
                        }
                        ColumnKind::Rid(target) => {
                            ensure!(
                                value == 0 || value <= tables.row_count(target),
                                "INVALID_INPUT: table index"
                            );
                        }
                        ColumnKind::Coded(coded) => {
                            let token = coded
                                .decode(value, 0)
                                .context("INVALID_INPUT: coded index")?;
                            if !token.is_null() {
                                let target = token.table().context("INVALID_INPUT: coded index")?;
                                ensure!(
                                    token.rid() <= tables.row_count(target),
                                    "INVALID_INPUT: coded index"
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn tables(&self) -> &cildec::Tables<'a> {
        self.metadata.tables()
    }

    /// Translate a file offset back to the RVA that maps it.
    pub(crate) fn offset_to_rva(&self, offset: usize) -> Result<u32> {
        let offset = u32::try_from(offset).context("INVALID_INPUT: file offset")?;
        ensure!(
            offset < self.bytes.len() as u32,
            "INVALID_INPUT: file offset"
        );
        if offset < self.image.size_of_headers() {
            return Ok(offset);
        }
        for section in self.image.sections() {
            let start = section.pointer_to_raw_data;
            let size = section.size_of_raw_data.min(section.mapped_size());
            if offset >= start && offset < start.saturating_add(size) {
                return section
                    .virtual_address
                    .checked_add(offset - start)
                    .context("INVALID_INPUT: RVA overflow");
            }
        }
        bail!("INVALID_INPUT: file offset outside mapped sections")
    }

    pub fn unreadable_reason(&self, token: u32) -> Option<&str> {
        self.unreadable
            .iter()
            .find(|row| row.token == Some(token))
            .map(|row| row.reason.as_str())
    }

    pub fn type_by_id(&self, id: &str) -> Result<&TypeInfo> {
        let token = self.entity_token(id, "type", 2)?;
        if let Some(reason) = self.unreadable_reason(token) {
            return Err(rxd_core::Error::failure(rxd_core::Failure::Input, reason).into());
        }
        let index = self
            .types
            .binary_search_by_key(&token, |ty| ty.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: type does not exist"))?;
        Ok(&self.types[index])
    }

    fn entity_token(&self, id: &str, kind: &str, table: u32) -> Result<u32> {
        let token = self.token_at(id)?;
        self.checked_token(token, kind, table)
    }

    fn checked_token(&self, token: u32, kind: &str, table: u32) -> Result<u32> {
        ensure!(
            token >> 24 == table,
            "INVALID_QUERY: token 0x{token:08x} is in table 0x{:02x}, not the `{kind}` table 0x{table:02x}; copy the `at` of a `{kind}` record from `rxd ilspy outline`",
            token >> 24,
        );
        // The token is well formed and names no row: that is absence, not a bad query.
        ensure!(
            token & 0xffffff > 0 && token & 0xffffff <= self.rows[table as usize],
            rxd_core::Error::failure(
                rxd_core::Failure::NotFound,
                format!("no {kind} at token 0x{token:08x} in this input"),
            ),
        );
        Ok(token)
    }

    /// Parse eight-digit metadata tokens with optional 0x; reject kind prefixes.
    fn token_at(&self, value: &str) -> Result<u32> {
        if let Some((kind, _)) = value.split_once(':') {
            bail!(
                "INVALID_QUERY: `{value}` carries a `{kind}:` prefix; a dotnet coordinate is the metadata token alone, for example `{}`",
                value.rsplit(':').next().unwrap_or(value)
            );
        }
        hex_token(value).context(
            "INVALID_QUERY: expected a metadata token such as `0x0600000b` (eight hex digits, `0x` optional)",
        )
    }

    /// Read MemberRef signature lazily to distinguish methods and fields.
    pub(crate) fn member_kind(&self, token: u32) -> Result<Option<bool>> {
        if token >> 24 != 10 {
            return Ok(None);
        }
        self.check_token(token, &[10])?;
        let row = self
            .tables()
            .member_ref(token & 0xffffff)
            .context("INVALID_INPUT: MemberRef row")?;
        let bytes = self.blob(row.signature)?;
        let first = bytes
            .first()
            .context("INVALID_INPUT: missing member signature")?;
        Ok(Some(*first != 6))
    }

    /// Resolve selected class metadata before constructing its immutable projection.
    pub fn prepare_class(&mut self, id: &str) -> Result<u32> {
        let token = self.type_by_id(id)?.token;
        crate::class::prepare(self, token)?;
        Ok(token)
    }

    pub fn prepare_class_fields(&mut self, root_token: u32) -> Result<()> {
        crate::class::prepare_fields(self, root_token)
    }

    pub fn class_body(&self, root_token: u32) -> Result<crate::ClassBody<'_>> {
        crate::class::build(self, root_token)
    }

    pub fn method_by_id(&self, id: &str) -> Result<&Method> {
        let token = self.entity_token(id, "method", 6)?;
        if let Some(reason) = self.unreadable_reason(token) {
            return Err(rxd_core::Error::failure(rxd_core::Failure::Input, reason).into());
        }
        let index = self
            .methods
            .binary_search_by_key(&token, |m| m.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: method does not exist"))?;
        Ok(&self.methods[index])
    }

    pub fn body(&mut self, method: &Method) -> Result<MethodBody> {
        let index = self
            .methods
            .binary_search_by_key(&method.token, |m| m.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: foreign method"))?;
        ensure!(
            self.types
                .binary_search_by_key(&method.declaring_type_token, |ty| ty.token)
                .is_ok(),
            "INVALID_QUERY: foreign method"
        );
        self.resolve_method(index)?;
        crate::il::body(self, &self.methods[index])
    }

    /// FETCH by token: resolve only this method's signature before decoding its body.
    pub fn body_token(&mut self, token: u32) -> Result<MethodBody> {
        if let Some(reason) = self.unreadable_reason(token) {
            return Err(rxd_core::Error::failure(rxd_core::Failure::Input, reason).into());
        }
        let index = self
            .methods
            .binary_search_by_key(&token, |m| m.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: method does not exist"))?;
        self.resolve_method(index)?;
        crate::il::body(self, &self.methods[index])
    }

    pub fn resolve_method_token(&mut self, token: u32) -> Result<()> {
        let index = self
            .methods
            .binary_search_by_key(&token, |method| method.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: method does not exist"))?;
        self.resolve_method(index)
    }

    pub fn resolve_field_token(&mut self, token: u32) -> Result<()> {
        let index = self
            .fields
            .binary_search_by_key(&token, |field| field.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: field does not exist"))?;
        self.resolve_field(index)
    }

    /// Seed signature work with metadata already consumed.
    fn signature_work(&self) -> Result<Work> {
        let mut work = Work::new(self.limits.work, self.limits.metadata_bytes);
        work.charge(self.metadata_work)?;
        Ok(work)
    }

    pub fn resolve_field(&mut self, index: usize) -> Result<()> {
        if self.fields[index].resolved {
            return Ok(());
        }
        let blob = self.fields[index].signature_index;
        let mut work = self.signature_work()?;
        let signature = Arc::new(crate::metadata::field_signature(
            self,
            self.blob(blob)?,
            &mut work,
        )?);
        let field = &mut self.fields[index];
        field.signature = signature;
        field.resolved = true;
        self.metadata_work = work.used();
        Ok(())
    }

    pub fn resolve_method(&mut self, index: usize) -> Result<()> {
        if self.methods[index].resolved {
            return Ok(());
        }
        let blob = self.methods[index].signature_index;
        let mut work = self.signature_work()?;
        let sig = crate::metadata::method_signature(self, self.blob(blob)?, &mut work)?;
        let method = &mut self.methods[index];
        method.signature = Arc::new(sig.display());
        method.parameters = sig.parameters.clone();
        method.return_type = sig.return_type.clone();
        method.has_this = sig.has_this;
        method.signature_flags = sig.flags;
        method.resolved = true;
        self.metadata_work = work.used();
        Ok(())
    }

    pub fn resolve_call(&self, token: u32, context: &Method) -> Result<CallSignature> {
        let mut work = Work::new(self.limits.work, self.limits.metadata_bytes);
        work.charge(self.metadata_work)?;
        let mut call = crate::metadata::call_signature(self, token, &mut work)?;
        if call.return_type.contains('!') || call.parameters.iter().any(|ty| ty.contains('!')) {
            let type_args =
                crate::metadata::type_arguments(self, call.declaring_type_token, &mut work)?;
            let same_owner = call.declaring_type_token == context.declaring_type_token;
            let mut replace = |ty: &str| {
                crate::metadata::instantiate(
                    ty,
                    type_args.as_deref(),
                    call.method_arguments.as_deref(),
                    same_owner,
                    &mut work,
                )
            };
            call.return_type = Arc::new(replace(&call.return_type)?);
            call.parameters = Arc::new(
                call.parameters
                    .iter()
                    .map(|ty| replace(ty))
                    .collect::<Result<_>>()?,
            );
        }
        Ok(call)
    }

    pub fn this_type(&self, method: &Method) -> Result<String> {
        let index = self
            .methods
            .binary_search_by_key(&method.token, |m| m.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: foreign method"))?;
        let canonical = &self.methods[index];
        ensure!(
            canonical.token == method.token,
            "INVALID_QUERY: foreign method"
        );
        let token = canonical.declaring_type_token;
        Ok(if canonical.declaring_type_is_value_type {
            format!("valuetype(0x{token:08x})&")
        } else {
            format!("class(0x{token:08x})")
        })
    }

    // TypeRef alone does not tell us whether a type is a class or value type.
    // Do not guess from its name or silently resolve a dependency.
    pub(crate) fn type_signature_with_work(&self, token: u32, work: &mut Work) -> Result<String> {
        work.charge(1)?;
        self.check_token(token, &[1, 2, 27])?;
        if let Some(value_type) = self.type_kinds.get(&token) {
            work.charge(if *value_type { 21 } else { 17 })?;
            return Ok(format!(
                "{}(0x{token:08x})",
                if *value_type { "valuetype" } else { "class" }
            ));
        }
        if token >> 24 == 27 {
            let row = self
                .tables()
                .type_spec(token & 0xffffff)
                .context("UNSUPPORTED: missing TypeSpec")?;
            return crate::metadata::type_signature(self, self.blob(row.signature)?, work);
        }
        bail!("UNSUPPORTED: unresolved type kind 0x{token:08x}")
    }

    pub(crate) fn field_signature_with_work(
        &self,
        token: u32,
        context: &Method,
        work: &mut Work,
    ) -> Result<(String, Option<bool>)> {
        work.charge(1)?;
        if let Ok(index) = self
            .fields
            .binary_search_by_key(&token, |field| field.token)
        {
            let field = &self.fields[index];
            // Unresolved rows parse their own blob here; this path never relies on the
            // eager projection having run for an unrelated member.
            let signature = if field.resolved {
                field.signature.as_ref().clone()
            } else {
                crate::metadata::field_signature(self, self.blob(field.signature_index)?, work)?
            };
            ensure!(
                !signature.contains('!')
                    || field.declaring_type_token == context.declaring_type_token,
                "UNSUPPORTED: unresolved field generic context"
            );
            work.charge(signature.len())?;
            return Ok((signature, Some(field.flags & 0x10 != 0)));
        }
        self.check_token(token, &[10])?;
        let row = self
            .tables()
            .member_ref(token & 0xffffff)
            .context("UNSUPPORTED: missing field MemberRef")?;
        let signature = crate::metadata::field_signature(self, self.blob(row.signature)?, work)?;
        let signature = if signature.contains('!') {
            let type_args = crate::metadata::type_arguments(self, row.class.value(), work)?;
            crate::metadata::instantiate(
                &signature,
                type_args.as_deref(),
                None,
                row.class.value() == context.declaring_type_token,
                work,
            )?
        } else {
            signature
        };
        Ok((signature, None))
    }

    pub fn strings(&self) -> Result<Vec<StringRecord>> {
        crate::metadata::strings(self)
    }

    pub fn string_uses(
        &mut self,
        contains: Option<&str>,
        case_sensitive: bool,
    ) -> Result<crate::StringUses> {
        crate::strings::uses(self, contains, case_sensitive)
    }

    pub fn string_references(
        &mut self,
        pattern: &str,
        exact: bool,
        case_sensitive: bool,
    ) -> Result<Vec<crate::StringReference>> {
        crate::strings::references(self, pattern, exact, case_sensitive)
    }

    pub fn user_string_references(&mut self, token: u32) -> Result<Vec<crate::StringReference>> {
        crate::strings::token_references(self, token)
    }

    pub fn validate_token(&self, token: u32) -> Result<()> {
        if token >> 24 == 0x70 {
            let requested = std::collections::BTreeSet::from([token & 0xffffff]);
            return crate::il::validate_user_strings(
                self,
                &requested,
                &mut Work::new(self.limits.work, self.limits.metadata_bytes),
            );
        }
        let table = (token >> 24) as u8;
        self.check_token(token, &[table])
    }

    pub(crate) fn check_token(&self, token: u32, tables: &[u8]) -> Result<()> {
        let table = (token >> 24) as u8;
        let rid = token & 0x00ff_ffff;
        ensure!(
            tables.contains(&table)
                && rid > 0
                && (table as usize) < self.rows.len()
                && rid <= self.rows[table as usize],
            "INVALID_INPUT: invalid metadata token 0x{token:08x}"
        );
        Ok(())
    }

    pub(crate) fn blob(&self, index: impl HeapIndex) -> Result<&[u8]> {
        let index = index.raw();
        ensure!(index != 0, "INVALID_INPUT: missing signature blob");
        self.metadata
            .blobs()
            .get(BlobIndex(index))
            .context("INVALID_INPUT: blob index")
    }

    pub(crate) fn name(&self, index: impl HeapIndex) -> Result<&str> {
        self.metadata
            .strings()
            .str(StringIndex(index.raw()))
            .context("INVALID_INPUT: string index")
    }

    pub fn type_name(&self, token: u32) -> Option<&str> {
        self.type_names.get(&token).map(|name| name.as_str())
    }

    /// Resolve names for definitions, references and generic TypeSpec tokens.
    pub fn type_display(&self, token: u32) -> Option<String> {
        if let Some(name) = self.type_name(token) {
            return Some(name.to_owned());
        }
        if token >> 24 == 27 {
            let mut work = Work::new(self.limits.work, self.limits.metadata_bytes);
            let text = self.type_signature_with_work(token, &mut work).ok()?;
            return Some(self.name_tokens(&text));
        }
        None
    }

    /// A signature spells a named type as `class(0x02000002)`; a row that prints a name
    /// replaces those with the names, keeping the surrounding constructed-type shape.
    fn name_tokens(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        loop {
            let hit = ["class(0x", "valuetype(0x"]
                .iter()
                .filter_map(|prefix| rest.find(prefix).map(|at| (at, prefix.len())))
                .min();
            let Some((at, prefix_len)) = hit else {
                out.push_str(rest);
                return out;
            };
            out.push_str(&rest[..at]);
            let digits = &rest[at + prefix_len..];
            let end = digits.find(')');
            let named = end
                .and_then(|end| u32::from_str_radix(&digits[..end], 16).ok())
                .and_then(|token| self.type_name(token));
            match (end, named) {
                (Some(end), Some(name)) => {
                    out.push_str(name);
                    rest = &digits[end + 1..];
                }
                _ => {
                    out.push_str(&rest[at..at + prefix_len]);
                    rest = digits;
                }
            }
        }
    }
}

/// A heap column: a stored index or the typed index of one row field.
pub(crate) trait HeapIndex: Copy {
    fn raw(self) -> u32;
}

impl HeapIndex for BlobIndex {
    fn raw(self) -> u32 {
        self.get()
    }
}

impl HeapIndex for StringIndex {
    fn raw(self) -> u32 {
        self.get()
    }
}

impl HeapIndex for u32 {
    fn raw(self) -> u32 {
        self
    }
}

/// The metadata token that names one row of `table`.
pub(crate) fn token_of(table: TableId, rid: u32) -> u32 {
    ((table.to_u8() as u32) << 24) | rid
}

fn hex_token(value: &str) -> Result<u32> {
    let body = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    // Require eight hex digits; a dropped digit can select a different metadata row.
    ensure!(
        body.len() == 8 && body.bytes().all(|b| b.is_ascii_hexdigit()),
        "`{value}` is not an 8-hex metadata token (expected e.g. `0600000b` or `0x0600000b`)"
    );
    u32::from_str_radix(body, 16).context("metadata token")
}

pub(crate) struct Work {
    used: usize,
    remaining: Option<usize>,
    metadata_reserved: usize,
    metadata_limit: usize,
}
impl Work {
    pub(crate) fn new(limit: Option<usize>, metadata_limit: usize) -> Self {
        Self {
            used: 0,
            remaining: limit,
            metadata_reserved: 0,
            metadata_limit,
        }
    }
    pub(crate) fn used(&self) -> usize {
        self.used
    }
    pub(crate) fn retain(&mut self, bytes: usize) -> Result<()> {
        let reserved = self
            .metadata_reserved
            .checked_add(bytes)
            .context("RESOURCE_LIMIT: metadata bytes")?;
        // Retained allocations are storage; repeated work is not resident memory.
        ensure!(
            reserved <= self.metadata_limit,
            "RESOURCE_LIMIT: metadata bytes"
        );
        self.metadata_reserved = reserved;
        Ok(())
    }
    pub(crate) fn charge(&mut self, amount: usize) -> Result<()> {
        if let Some(remaining) = self.remaining.as_mut() {
            *remaining = remaining
                .checked_sub(amount)
                .context("RESOURCE_LIMIT: analysis work")?;
        }
        self.used = self
            .used
            .checked_add(amount)
            .context("RESOURCE_LIMIT: work counter overflow")?;
        Ok(())
    }
}

// Account backend-retained bytes, not parser/native-engine RSS.
pub(crate) struct WorkingSet {
    used: usize,
    limit: Option<usize>,
    what: &'static str,
}

impl WorkingSet {
    pub(crate) fn new(limit: impl Into<Option<usize>>, what: &'static str) -> Self {
        Self {
            used: 0,
            limit: limit.into(),
            what,
        }
    }

    /// Count `bytes` before the caller allocates them. Exhaustion is a resource
    /// failure, never a truncated result.
    pub(crate) fn add(&mut self, bytes: usize) -> Result<()> {
        let next = self
            .used
            .checked_add(bytes)
            .with_context(|| format!("RESOURCE_LIMIT: {} size", self.what))?;
        ensure!(
            self.limit.is_none_or(|limit| next <= limit),
            "RESOURCE_LIMIT: {}",
            self.what
        );
        self.used = next;
        Ok(())
    }
}

#[cfg(test)]
mod working_set_tests {
    use super::WorkingSet;

    #[test]
    fn working_set_is_an_exact_bound_and_reports_the_operation() {
        let mut set = WorkingSet::new(10, "test working set");
        set.add(4).unwrap();
        set.add(6).unwrap(); // exact fit succeeds
        let error = set.add(1).unwrap_err().to_string();
        assert!(error.contains("RESOURCE_LIMIT"), "{error}");
        assert!(error.contains("test working set"), "{error}");
        // A refused add leaves the accumulated footprint unchanged: zero still fits.
        set.add(0).unwrap();

        let mut overflow = WorkingSet::new(usize::MAX, "overflow working set");
        overflow.add(1).unwrap();
        let error = overflow.add(usize::MAX).unwrap_err().to_string();
        assert!(error.contains("RESOURCE_LIMIT"), "{error}");
        assert!(error.contains("overflow working set"), "{error}");
    }
}
