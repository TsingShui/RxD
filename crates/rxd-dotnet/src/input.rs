use std::{collections::BTreeMap, fs::OpenOptions, io::Read, path::Path, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use dotscope::{CilAssemblyView, ValidationConfig};

use crate::{CallSignature, Field, Limits, Method, MethodBody, StringRecord, TypeInfo};

pub struct Input {
    pub source_id: String,
    pub file_bytes: usize,
    pub types: Vec<TypeInfo>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub properties: Vec<crate::Property>,
    pub events: Vec<crate::Event>,
    pub(crate) view: CilAssemblyView,
    pub(crate) limits: Limits,
    pub(crate) rows: [u32; 64],
    pub(crate) metadata_work: usize,
    pub(crate) type_kinds: BTreeMap<u32, bool>,
    pub(crate) relationships_loaded: bool,
    /// Metadata rows skipped because their own contents were invalid.
    pub unreadable: Vec<crate::Unreadable>,
    /// Metadata token to full type name, for the tokens a row can point at but not show:
    /// a base type or an interface is a token, and a name is what a reader navigates by.
    pub(crate) type_names: BTreeMap<u32, Arc<String>>,
}

impl Input {
    pub fn read(path: &Path, limits: Limits) -> Result<Self> {
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
        let file_bytes = bytes.len();
        let source_id = rxd_core::identity::input_id(&bytes);
        let parsed_file = dotscope::File::from_mem(bytes).context("INVALID_INPUT: PE parsing")?;
        ensure!(
            parsed_file.repairs().is_empty(),
            "INVALID_INPUT: automatic PE repair is unsupported"
        );
        // PE/stream/table headers are parsed here; whole-module cross-table validation is
        // not run for a query. Every row, heap entry and token this query actually reads
        // is bounds/dialect checked at the point of use, so unrelated corruption cannot
        // turn a local answer into a whole-file failure. `verify_module` keeps an explicit
        // full-structure check for callers that want it.
        let mut deferred = ValidationConfig::minimal();
        deferred.enable_cross_table_validation = true;
        deferred.enable_raw_validation = false;
        let view = CilAssemblyView::from_dotscope_file_with_validation(parsed_file, deferred)
            .context("INVALID_INPUT: metadata structure and references")?;
        let mut rows = [0; 64];
        let mut total = 0usize;
        let tables = view
            .tables()
            .context("INVALID_INPUT: missing metadata tables")?;
        for summary in tables.table_summary() {
            let idx = summary.table_id as usize;
            ensure!(idx < rows.len(), "INVALID_INPUT: table index");
            rows[idx] = summary.row_count;
            total = total
                .checked_add(summary.row_count as usize)
                .context("RESOURCE_LIMIT: row count")?;
        }
        ensure!(total <= limits.work, "RESOURCE_LIMIT: metadata work");
        let mut input = Self {
            source_id,
            file_bytes,
            types: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
            events: Vec::new(),
            view,
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

    /// Opt-in full-structure check (cross-table/heap/token validation). Normal queries
    /// validate only what they read.
    pub fn verify_module(&self) -> Result<()> {
        let mut config = ValidationConfig::minimal();
        config.enable_cross_table_validation = true;
        crate::validation::validate(&self.view, config)
            .context("INVALID_INPUT: metadata structure and references")
    }

    /// Why this token's row is unusable, when index construction skipped it.
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

    /// Resolve a coordinate, or the token form this input already fixes:
    /// `method:06000003`, `06000003` or `0x06000003`. A short form names a token in
    /// the validated buffer, so content identity still comes from the parsed input.
    fn entity_token(&self, id: &str, kind: &str, table: u32) -> Result<u32> {
        let token = self.token_at(id)?;
        self.checked_token(token, kind, table)
    }

    fn checked_token(&self, token: u32, kind: &str, table: u32) -> Result<u32> {
        ensure!(
            token >> 24 == table,
            "INVALID_QUERY: token 0x{token:08x} is in table 0x{:02x}, not the `{kind}` table 0x{table:02x}; copy the `at` of a `{kind}` record from `rxd dotnet outline`",
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

    /// Parse the coordinate an Agent copies out of a record: the ECMA-335 metadata
    /// token, eight hex digits with an optional `0x`. A kind prefix is refused with
    /// the value to use instead, because the token's high byte already names the table.
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

    /// Whether a `MemberRef` token denotes a method or a field, read from its signature
    /// blob only when a query actually needs the distinction.
    pub(crate) fn member_kind(&self, token: u32) -> Result<Option<bool>> {
        if token >> 24 != 10 {
            return Ok(None);
        }
        self.check_token(token, &[10])?;
        use dotscope::metadata::tables::MemberRefRaw;
        let row = self
            .view
            .tables()
            .and_then(|tables| tables.table::<MemberRefRaw>())
            .context("INVALID_INPUT: MemberRef table")?
            .get(token & 0xffffff)?
            .context("INVALID_INPUT: MemberRef row")?;
        let bytes = self.blob(row.signature)?;
        let first = bytes
            .first()
            .context("INVALID_INPUT: missing member signature")?;
        Ok(Some(*first != 6))
    }

    /// Resolve the class-scoped metadata first, then build the projection immutably
    /// so callers can keep using the rest of this input while it is alive.
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

    /// Resolve one row's signature by its metadata token.
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

    /// Signature handling is the same for every caller: seed a work scope with the
    /// metadata already spent, parse one blob, and keep the consumed work visible.
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
            use dotscope::metadata::tables::TypeSpecRaw;
            let row = self
                .view
                .tables()
                .and_then(|t| t.table::<TypeSpecRaw>())
                .context("UNSUPPORTED: missing TypeSpec table")?
                .get(token & 0xffffff)?
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
        use dotscope::metadata::tables::MemberRefRaw;
        let row = self
            .view
            .tables()
            .and_then(|t| t.table::<MemberRefRaw>())
            .context("UNSUPPORTED: missing MemberRef table")?
            .get(token & 0xffffff)?
            .context("UNSUPPORTED: missing field MemberRef")?;
        let signature = crate::metadata::field_signature(self, self.blob(row.signature)?, work)?;
        let signature = if signature.contains('!') {
            let type_args = crate::metadata::type_arguments(self, row.class.token.value(), work)?;
            crate::metadata::instantiate(
                &signature,
                type_args.as_deref(),
                None,
                row.class.token.value() == context.declaring_type_token,
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

    pub(crate) fn blob(&self, index: u32) -> Result<&[u8]> {
        ensure!(index != 0, "INVALID_INPUT: missing signature blob");
        self.view
            .blobs()
            .context("INVALID_INPUT: missing blob heap")?
            .get(index as usize)
            .context("INVALID_INPUT: blob index")
    }

    pub(crate) fn name(&self, index: u32) -> Result<&str> {
        let name = self
            .view
            .strings()
            .context("INVALID_INPUT: missing string heap")?
            .get(index as usize)
            .context("INVALID_INPUT: string index")?;
        Ok(name)
    }

    /// The full name a metadata token points at, when this assembly knows it.
    pub fn type_name(&self, token: u32) -> Option<&str> {
        self.type_names.get(&token).map(|name| name.as_str())
    }

    /// A printable type for any token a row can name: a definition, a reference, or a
    /// generic instantiation. A row prints the token as its identity and this as the name
    /// a reader navigates by, so a TypeSpec must resolve too, not just fall through null.
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

/// Parse `06000003` or `0x06000003`; every CIL token is eight hex digits.
fn hex_token(value: &str) -> Result<u32> {
    let body = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    // ECMA-335 tokens are exactly eight hex digits (table byte + three-byte row).
    // A dropped or extra digit can land on another row, so length is enforced
    // instead of letting a mis-copied token resolve to a different member.
    ensure!(
        body.len() == 8 && body.bytes().all(|b| b.is_ascii_hexdigit()),
        "`{value}` is not an 8-hex metadata token (expected e.g. `0600000b` or `0x0600000b`)"
    );
    u32::from_str_radix(body, 16).context("metadata token")
}

pub(crate) struct Work {
    initial: usize,
    remaining: usize,
    metadata_reserved: usize,
    metadata_limit: usize,
}
impl Work {
    pub(crate) fn new(limit: usize, metadata_limit: usize) -> Self {
        Self {
            initial: limit,
            remaining: limit,
            metadata_reserved: 0,
            metadata_limit,
        }
    }
    pub(crate) fn used(&self) -> usize {
        self.initial - self.remaining
    }

    // Retained containers are a byte budget, not repeated parsing work. Include
    // charged work as a conservative allowance for decoded/copied string payloads.
    pub(crate) fn retain(&mut self, bytes: usize) -> Result<()> {
        let reserved = self
            .metadata_reserved
            .checked_add(bytes)
            .context("RESOURCE_LIMIT: metadata bytes")?;
        ensure!(
            reserved.saturating_add(self.used()) <= self.metadata_limit,
            "RESOURCE_LIMIT: metadata bytes"
        );
        self.metadata_reserved = reserved;
        Ok(())
    }

    pub(crate) fn charge(&mut self, amount: usize) -> Result<()> {
        if amount > self.remaining {
            bail!("RESOURCE_LIMIT: analysis work");
        }
        ensure!(
            self.metadata_reserved
                .saturating_add(self.used())
                .saturating_add(amount)
                <= self.metadata_limit,
            "RESOURCE_LIMIT: metadata bytes"
        );
        self.remaining -= amount;
        Ok(())
    }
}

// Decoded/retained working memory for one operation. The accounting is exact about
// what this backend itself allocates; it is not an RSS bound for the parser or the
// pinned native engine.
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
