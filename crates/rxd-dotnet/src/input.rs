use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::OpenOptions, io::Read, path::Path};

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
    pub(crate) calls: BTreeMap<u32, CallSignature>,
    pub(crate) member_kinds: BTreeMap<u32, bool>,
    pub(crate) type_kinds: BTreeMap<u32, bool>,
    /// Metadata token to full type name, for the tokens a row can point at but not show:
    /// a base type or an interface is a token, and a name is what a reader navigates by.
    pub(crate) type_names: BTreeMap<u32, String>,
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
        let source_id = rxd_core::identity::captured_id(path, bytes.len()).unwrap_or_else(|| {
            let digest = Sha256::digest(&bytes);
            let mut source_id = String::with_capacity(64);
            const HEX: &[u8; 16] = b"0123456789abcdef";
            for byte in digest {
                source_id.push(HEX[(byte >> 4) as usize] as char);
                source_id.push(HEX[(byte & 15) as usize] as char);
            }
            source_id
        });
        let parsed_file = dotscope::File::from_mem(bytes).context("INVALID_INPUT: PE parsing")?;
        ensure!(
            parsed_file.repairs().is_empty(),
            "INVALID_INPUT: automatic PE repair is unsupported"
        );
        // Keep structural, heap and metadata-token checks, while avoiding unrelated
        // whole-assembly semantic/signature passes. Queried signatures and IL tokens
        // are separately validated with bounded walkers in this backend.
        let mut validation = ValidationConfig::minimal();
        validation.enable_cross_table_validation = true;
        // Defer the validation engine until raw table counts pass the budgets.
        // The raw view constructor still performs bounded PE/stream/table parsing;
        // the complete query structural/reference validation runs below.
        let mut deferred = validation;
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
        crate::validation::validate(&view, validation)
            .context("INVALID_INPUT: metadata structure and references")?;
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
            calls: BTreeMap::new(),
            member_kinds: BTreeMap::new(),
            type_kinds: BTreeMap::new(),
            type_names: BTreeMap::new(),
        };
        crate::metadata::load(&mut input)?;
        Ok(input)
    }

    pub fn type_by_id(&self, id: &str) -> Result<&TypeInfo> {
        let token = self.entity_token(id, "type", 2)?;
        let index = self
            .types
            .binary_search_by_key(&token, |ty| ty.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: type does not exist"))?;
        Ok(&self.types[index])
    }

    /// Resolve a content-bound id, or the short form this input already fixes:
    /// `method:06000003`, `06000003` or `0x06000003`. A short form names a token in
    /// the validated buffer, so content identity still comes from the parsed input.
    fn entity_token(&self, id: &str, kind: &str, table: u32) -> Result<u32> {
        let token = match self.token_from_id(id, kind)? {
            Some(token) => token,
            None => self.token_at(id)?,
        };
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

    /// Strict `rxd1:dotnet:<sha256>:<kind>:<token>` parsing that names the part
    /// which does not match, instead of one opaque "mismatch".
    fn token_from_id(&self, id: &str, kind: &str) -> Result<Option<u32>> {
        let Some(rest) = id.strip_prefix("rxd1:") else {
            return Ok(None);
        };
        let mut parts = rest.split(':');
        let backend = parts.next().unwrap_or_default();
        ensure!(
            backend == "dotnet",
            "INVALID_QUERY: id belongs to backend `{backend}`; `rxd dotnet` needs a `dotnet` id"
        );
        let hash = parts.next().unwrap_or_default();
        ensure!(
            hash == self.source_id,
            "INVALID_QUERY: id belongs to a different input (id sha256 {}…, this input {}…); re-run `outline` for this file",
            short_hash(hash),
            short_hash(&self.source_id),
        );
        let actual = parts.next().unwrap_or_default();
        ensure!(
            actual == kind,
            "INVALID_QUERY: id is a `{actual}` id but this command needs a `{kind}` id"
        );
        let encoded = parts.next().unwrap_or_default();
        ensure!(
            parts.next().is_none(),
            "INVALID_QUERY: malformed id: unexpected segments after `{encoded}`"
        );
        Ok(Some(
            hex_token(encoded).context("INVALID_QUERY: malformed id token")?,
        ))
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

    pub fn class_body(&self, id: &str) -> Result<crate::ClassBody<'_>> {
        crate::class::body(self, self.type_by_id(id)?)
    }

    pub fn method_by_id(&self, id: &str) -> Result<&Method> {
        let token = self.entity_token(id, "method", 6)?;
        let index = self
            .methods
            .binary_search_by_key(&token, |m| m.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: method does not exist"))?;
        Ok(&self.methods[index])
    }

    pub fn body(&self, method: &Method) -> Result<MethodBody> {
        let index = self
            .methods
            .binary_search_by_key(&method.token, |m| m.token)
            .map_err(|_| anyhow::anyhow!("INVALID_QUERY: foreign method"))?;
        let canonical = &self.methods[index];
        ensure!(canonical.id == method.id, "INVALID_QUERY: foreign method");
        crate::il::body(self, canonical)
    }

    pub fn resolve_call(&self, token: u32, context: &Method) -> Result<CallSignature> {
        let mut call = if token >> 24 == 6 {
            let index = self
                .methods
                .binary_search_by_key(&token, |method| method.token)
                .map_err(|_| anyhow::anyhow!("INVALID_INPUT: missing method call token"))?;
            let method = &self.methods[index];
            ensure!(
                !matches!(method.signature_flags & 15, 5 | 11),
                "UNSUPPORTED: vararg call signature"
            );
            method_call(method)
        } else {
            self.calls
                .get(&token)
                .cloned()
                .with_context(|| format!("UNSUPPORTED: unresolved call signature 0x{token:08x}"))?
        };
        if call.return_type.contains('!') || call.parameters.iter().any(|ty| ty.contains('!')) {
            let mut work = Work::new(self.limits.work);
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
            call.return_type = replace(&call.return_type)?;
            call.parameters = call
                .parameters
                .iter()
                .map(|ty| replace(ty))
                .collect::<Result<_>>()?;
        }
        Ok(call)
    }

    pub fn this_type(&self, method: &Method) -> Result<String> {
        let canonical = self.method_by_id(&method.id)?;
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
            ensure!(
                !field.signature.contains('!')
                    || field.declaring_type_token == context.declaring_type_token,
                "UNSUPPORTED: unresolved field generic context"
            );
            work.charge(field.signature.len())?;
            return Ok((field.signature.clone(), Some(field.flags & 0x10 != 0)));
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
        &self,
        contains: Option<&str>,
        case_sensitive: bool,
    ) -> Result<crate::StringUses> {
        crate::strings::uses(self, contains, case_sensitive)
    }

    pub fn string_references(
        &self,
        pattern: &str,
        exact: bool,
        case_sensitive: bool,
    ) -> Result<Vec<crate::StringReference>> {
        crate::strings::references(self, pattern, exact, case_sensitive)
    }

    pub fn user_string_references(&self, token: u32) -> Result<Vec<crate::StringReference>> {
        crate::strings::token_references(self, token)
    }

    pub fn validate_token(&self, token: u32) -> Result<()> {
        if token >> 24 == 0x70 {
            let requested = std::collections::BTreeSet::from([token & 0xffffff]);
            return crate::il::validate_user_strings(
                self,
                &requested,
                &mut Work::new(self.limits.work),
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
        self.type_names.get(&token).map(String::as_str)
    }

    /// A printable type for any token a row can name: a definition, a reference, or a
    /// generic instantiation. A row prints the token as its identity and this as the name
    /// a reader navigates by, so a TypeSpec must resolve too, not just fall through null.
    pub fn type_display(&self, token: u32) -> Option<String> {
        if let Some(name) = self.type_name(token) {
            return Some(name.to_owned());
        }
        if token >> 24 == 27 {
            let mut work = Work::new(self.limits.work);
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

    pub(crate) fn id(&self, kind: &str, token: u32) -> String {
        format!("rxd1:dotnet:{}:{kind}:{token:08x}", self.source_id)
    }
}

/// First 12 hex digits of a content hash, enough to tell inputs apart in a message.
fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
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
}
impl Work {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            initial: limit,
            remaining: limit,
        }
    }
    pub(crate) fn used(&self) -> usize {
        self.initial - self.remaining
    }

    pub(crate) fn charge(&mut self, amount: usize) -> Result<()> {
        if amount > self.remaining {
            bail!("RESOURCE_LIMIT: analysis work");
        }
        self.remaining -= amount;
        Ok(())
    }
}

// Method definitions already own their full bounded signature. Build a call
// projection only when queried, rather than duplicating every signature in an
// eager second index on every outline/get request.
pub(crate) fn method_call(method: &Method) -> CallSignature {
    CallSignature {
        token: method.token,
        name: format!("{}::{}", method.declaring_type, method.name),
        parameters: method.parameters.clone(),
        return_type: method.return_type.clone(),
        has_this: method.has_this,
        signature_flags: method.signature_flags,
        declaring_type_token: method.declaring_type_token,
        method_arguments: None,
    }
}
