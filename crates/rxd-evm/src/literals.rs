//! Explicit decoded PUSH-immediate evidence and owner routes. No runtime string inference.
use crate::{
    BoundedOutput, FunctionNavigation, Limits, Navigation, hex,
    navigation::{Listing, object, work, write_record},
    references::{self, HeaderDetail, SummaryDetail},
};
use alloy_primitives::U256;
use rxd_core::{
    Error, Result,
    envelope::{self, RecordMeta},
    perf::Perf,
};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LiteralKind {
    Text,
    Bytes,
    Number,
}

pub struct LiteralQuery {
    pub kind: LiteralKind,
    pub(crate) normalized: Option<String>,
    prefix: Vec<usize>,
    pub(crate) number: Option<U256>,
}

impl LiteralQuery {
    pub fn new(kind: LiteralKind, query: Option<&str>) -> Result<Self> {
        let invalid = || {
            Error::new(
                "INVALID_QUERY",
                "expected nonempty text, even-width ASCII hex bytes, or a decimal/0x unsigned 256-bit number",
            )
        };
        let mut number = None;
        let normalized = match (kind, query) {
            (_, None) => None,
            (LiteralKind::Text, Some(text)) => {
                if text.is_empty() {
                    return Err(invalid());
                }
                Some(text.to_owned())
            }
            (LiteralKind::Bytes, Some(bytes)) => {
                let bytes = bytes
                    .strip_prefix("0x")
                    .or_else(|| bytes.strip_prefix("0X"))
                    .unwrap_or(bytes);
                if !bytes.len().is_multiple_of(2) || !bytes.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(invalid());
                }
                Some(bytes.to_ascii_lowercase())
            }
            (LiteralKind::Number, Some(value)) => {
                let (digits, radix) = if let Some(digits) = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                {
                    (digits, 16)
                } else {
                    (value, 10)
                };
                if digits.is_empty()
                    || !digits.bytes().all(|b| {
                        if radix == 16 {
                            b.is_ascii_hexdigit()
                        } else {
                            b.is_ascii_digit()
                        }
                    })
                {
                    return Err(invalid());
                }
                let parsed = U256::from_str_radix(digits, radix).map_err(|_| invalid())?;
                number = Some(parsed);
                Some(format!("{parsed:064x}"))
            }
        };
        let mut prefix = vec![0; normalized.as_ref().map_or(0, String::len)];
        if let Some(needle) = &normalized {
            let bytes = needle.as_bytes();
            for i in 1..bytes.len() {
                let mut j = prefix[i - 1];
                while j > 0 && bytes[i] != bytes[j] {
                    j = prefix[j - 1];
                }
                if bytes[i] == bytes[j] {
                    j += 1;
                }
                prefix[i] = j;
            }
        }
        Ok(Self {
            prefix,
            kind,
            normalized,
            number,
        })
    }

    pub(crate) fn text_matches(
        &self,
        navigation: &Navigation,
        start: usize,
        bytes: &[u8],
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Vec<Value>> {
        let needle = self.normalized.as_ref().expect("text query").as_bytes();
        let mut matched = 0;
        let mut matches = Vec::new();
        for (i, &byte) in bytes.iter().enumerate() {
            work(perf, 1, limits.work)?;
            while matched > 0 && byte != needle[matched] {
                work(perf, 1, limits.work)?;
                matched = self.prefix[matched - 1];
            }
            if byte == needle[matched] {
                matched += 1;
            }
            if matched == needle.len() {
                work(perf, 1, limits.work)?;
                if (matches.len() + 1).saturating_mul(64) > limits.output_bytes {
                    return Err(Error::new(
                        "RESOURCE_LIMIT",
                        "EVM matched evidence byte budget exceeded",
                    ));
                }
                let begin = start + i + 1 - needle.len();
                let end = start + i + 1;
                let (cs, ce) = navigation.carrier.carrier_range(begin, end);
                matches.push(json!({"decoded_start":begin,"decoded_end":end,"carrier_start":cs,"carrier_end":ce}));
                matched = self.prefix[matched - 1];
            }
        }
        Ok(matches)
    }

    fn mode(&self) -> &'static str {
        if self.normalized.is_none() {
            "inventory"
        } else {
            match self.kind {
                LiteralKind::Text => "contains",
                LiteralKind::Bytes => "equal_bytes",
                LiteralKind::Number => "equal_u256",
            }
        }
    }

    fn json(&self, navigation: &Navigation) -> Value {
        json!({"encoding": navigation.carrier.encoding, "kind": self.kind, "query": self.normalized, "match_mode": self.mode()})
    }
}

struct Literal {
    /// Zero-based position in the decoded-instruction inventory; stable per stream.
    index: usize,
    instruction: usize,
    block: usize,
    width: usize,
}

/// The copy-only value a literal row is named by.
///
/// Decoded text when the whole operand is one printable run, otherwise the lowercase hex
/// of its bytes. A zero-width `PUSH0` operand has no bytes and therefore an empty value.
pub(crate) fn literal_value(bytes: &[u8]) -> String {
    let mut runs = crate::text::printable_text_runs(bytes);
    match (runs.next(), runs.next()) {
        (Some((0, text)), None) if text.len() == bytes.len() => text.to_owned(),
        _ => hex(bytes),
    }
}

struct LiteralMatch<'a> {
    literal: &'a Literal,
    candidates: Vec<Value>,
    matches: Vec<Value>,
}

struct SearchEvidence<'a> {
    query: &'a LiteralQuery,
    candidates: &'a [Value],
    matches: &'a [Value],
}

pub struct LiteralInventory {
    literals: Vec<Literal>,
    excluded_unindexed_pushes: usize,
}

// PUSH operands are at most 32 bytes; data candidates retain their separate storage budget.
fn text_candidates(bytes: &[u8], start: usize) -> Vec<Value> {
    crate::text::printable_text_runs(bytes)
        .map(|(offset, text)| {
            json!({"decoded_start": start + offset,
            "decoded_end": start + offset + text.len(), "text": text})
        })
        .collect()
}

impl LiteralInventory {
    pub fn discover(navigation: &Navigation, limits: Limits, perf: &mut Perf) -> Result<Self> {
        let stage = perf.start_stage();
        let result = (|| {
            let mut literals = Vec::new();
            let mut excluded_unindexed_pushes = 0;
            for (index, instruction) in navigation.instructions.iter().enumerate() {
                work(perf, 1, limits.work)?;
                if !(0x5f..=0x7f).contains(&instruction.opcode) {
                    continue;
                }
                let Some(block) = navigation
                    .blocks
                    .partition_point(|b| b.start <= instruction.decoded_offset)
                    .checked_sub(1)
                    .filter(|&b| instruction.decoded_end <= navigation.blocks[b].end)
                else {
                    excluded_unindexed_pushes += 1;
                    continue;
                };
                literals.push(Literal {
                    index: literals.len(),
                    instruction: index,
                    block,
                    width: usize::from(instruction.opcode - 0x5f),
                });
            }
            Ok(Self {
                literals,
                excluded_unindexed_pushes,
            })
        })();
        perf.record("search", stage);
        result
    }

    /// The copy-only coordinate a row prints and `--at` accepts.
    fn coordinate(&self, navigation: &Navigation, literal: &Literal) -> String {
        format!(
            "{}:literal:push:{}:{}",
            navigation.carrier.encoding.as_str(),
            navigation.instructions[literal.instruction].decoded_offset,
            literal.width
        )
    }

    fn id(&self, navigation: &Navigation, literal: &Literal) -> String {
        format!(
            "rxd1:evm:{}:{}",
            navigation.carrier.source_id,
            self.coordinate(navigation, literal)
        )
    }

    fn select<'a>(&'a self, navigation: &Navigation, at: &str) -> Result<&'a Literal> {
        let invalid = || {
            Error::new(
                "INVALID_ID",
                "expected a canonical content-bound EVM PUSH literal ID",
            )
        };
        let canonical = crate::canonical_id(&navigation.carrier.source_id, at);
        let fields: Vec<_> = canonical.split(':').collect();
        if fields.len() != 8
            || fields[0] != "rxd1"
            || fields[1] != "evm"
            || fields[4] != "literal"
            || fields[5] != "push"
            || fields[2].len() != 64
            || !fields[2]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !matches!(fields[3], "raw" | "hex")
        {
            return Err(invalid());
        }
        let offset = fields[6].parse::<usize>().map_err(|_| invalid())?;
        let width = fields[7].parse::<usize>().map_err(|_| invalid())?;
        if offset.to_string() != fields[6] || width.to_string() != fields[7] || width > 32 {
            return Err(invalid());
        }
        if fields[2] != navigation.carrier.source_id
            || fields[3] != navigation.carrier.encoding.as_str()
        {
            return Err(Error::new(
                "SOURCE_MISMATCH",
                "EVM literal ID does not match the carrier and encoding",
            ));
        }
        self.literals
            .iter()
            .find(|l| {
                navigation.instructions[l.instruction].decoded_offset == offset && l.width == width
            })
            .ok_or_else(invalid)
    }

    fn routes(functions: &FunctionNavigation, literal: &Literal) -> Vec<Option<usize>> {
        if functions.owners[literal.block].is_empty() {
            vec![None]
        } else {
            functions.owners[literal.block]
                .iter()
                .map(|&owner| Some(owner))
                .collect()
        }
    }

    fn record(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        literal: &Literal,
        owner: Option<usize>,
        search: Option<SearchEvidence<'_>>,
    ) -> Value {
        let instruction = &navigation.instructions[literal.instruction];
        let operand_start = instruction.decoded_offset + 1;
        let operand_end = instruction.decoded_end;
        let bytes = &navigation.carrier.bytecode[operand_start..operand_end];
        let id = self.id(navigation, literal);
        let block_id = navigation.block_id(&navigation.blocks[literal.block]);
        let context_id = owner
            .map(|i| functions.function_id(navigation, &functions.functions[i]))
            .unwrap_or_else(|| block_id.clone());
        let (start, end) = navigation.carrier.carrier_range(operand_start, operand_end);
        let (instruction_start, instruction_end) = navigation
            .carrier
            .carrier_range(instruction.decoded_offset, instruction.decoded_end);
        let mut detail = object(
            json!({"literal_kind": "push_immediate", "instruction_offset": instruction.decoded_offset,
            "instruction_end": instruction.decoded_end, "opcode": instruction.opcode, "width_bytes": literal.width,
            "decoded_start": operand_start, "decoded_end": operand_end, "bytes_hex": hex(bytes),
            "number_hex": format!("{:064x}", U256::from_be_slice(bytes)), "block_id": navigation.block_coordinate(&navigation.blocks[literal.block]),
            "owner_count": functions.owners[literal.block].len(), "context_kind": if owner.is_some() { "function" } else { "block" },
            "context_boundary_status": owner.map(|i| if functions.functions[i].boundary_complete() { "static_reachable" } else { "incomplete" }).unwrap_or("producer_candidate"),
            "context_boundary_complete": owner.map(|i| functions.functions[i].boundary_complete()),
            "runtime_string_proven": false}),
        );
        if let Some(SearchEvidence {
            query,
            candidates,
            matches,
        }) = &search
        {
            detail.extend(object(json!({"value": literal_value(bytes), "index": literal.index,
                "query_kind": query.kind, "evidence_kind": if matches!(query.kind, LiteralKind::Text) { "push_immediate_text_candidate" } else { "push_immediate" },
                "match_status": if query.normalized.is_some() { "matched" } else { "inventoried" }, "text_candidates": candidates, "matches": matches})));
        } else {
            detail.extend(object(json!({"target": self.coordinate(navigation, literal), "evidence_kind": "decoded_push_use", "reference_kind": "push_immediate", "instruction_carrier_start": instruction_start, "instruction_carrier_end": instruction_end})));
        }
        envelope::record(
            if search.is_some() {
                "literal"
            } else {
                "literal_reference"
            },
            RecordMeta {
                at: search.map(|_| id.as_str()),
                context_at: Some(&context_id),
                start: Some(start),
                end: Some(end),
                ..RecordMeta::default()
            },
            detail,
        )
    }

    fn header(&self, navigation: &Navigation, command: &str, query: Value, include: bool) -> Value {
        envelope::header(
            "evm",
            command,
            Some(&navigation.carrier.source_id),
            Some(navigation.carrier.bytes.len()),
            query,
            crate::navigation::header_detail(object(
                json!({"carrier": navigation.carrier_detail(), "bytecode": navigation.bytecode_detail(include), "offset_unit": "decoded_bytecode_byte", "literal_scope": "validated_block_push_immediates", "excluded_unindexed_pushes": self.excluded_unindexed_pushes, "discovery_complete": null, "disassembly_complete": navigation.decoded_end == navigation.carrier.bytecode.len(), "undecoded_tail": navigation.undecoded_tail()}),
            )),
        )
    }

    fn search_matches<'a>(
        &'a self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        query: &LiteralQuery,
        filter: Option<&str>,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<(Vec<LiteralMatch<'a>>, usize)> {
        work(
            perf,
            query.normalized.as_ref().map_or(0, String::len),
            limits.work,
        )?;
        let mut records = 0usize;
        let mut found = Vec::new();
        for literal in &self.literals {
            let instruction = &navigation.instructions[literal.instruction];
            let start = instruction.decoded_offset + 1;
            let bytes = &navigation.carrier.bytecode[start..instruction.decoded_end];
            work(perf, 1 + bytes.len(), limits.work)?;
            let candidates = if matches!(query.kind, LiteralKind::Text) {
                text_candidates(bytes, start)
            } else {
                Vec::new()
            };
            let mut matches = Vec::new();
            let matched = match query.kind {
                LiteralKind::Text => {
                    for candidate in &candidates {
                        let text = candidate["text"].as_str().unwrap();
                        let begin = candidate["decoded_start"].as_u64().unwrap() as usize;
                        if query.normalized.is_some() {
                            matches.extend(query.text_matches(
                                navigation,
                                begin,
                                text.as_bytes(),
                                limits,
                                perf,
                            )?);
                        }
                    }
                    if query.normalized.is_none() {
                        !candidates.is_empty()
                    } else {
                        !matches.is_empty()
                    }
                }
                LiteralKind::Bytes => query
                    .normalized
                    .as_ref()
                    .is_none_or(|needle| *needle == instruction.operand_hex),
                LiteralKind::Number => query
                    .number
                    .is_none_or(|number| number == U256::from_be_slice(bytes)),
            };
            if !matched {
                continue;
            }
            if filter.is_some_and(|needle| !literal_value(bytes).to_lowercase().contains(needle)) {
                continue;
            }
            let route_count = functions.owners[literal.block].len().max(1);
            work(perf, route_count, limits.work)?;
            records += route_count;
            perf.counters.records = Some(records);
            if records > limits.records {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "EVM result record budget exceeded",
                ));
            }
            // Every fixed envelope alone exceeds this conservative lower bound.
            // Bound prepared match storage before exact bounded serialization.
            if records.saturating_mul(128) > limits.output_bytes {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "EVM output byte budget exceeded",
                ));
            }
            found.push(LiteralMatch {
                literal,
                candidates,
                matches,
            });
        }
        perf.counters.records = Some(records);
        perf.counters.matches = Some(found.len());
        Ok((found, records))
    }

    pub fn strings_json(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        query: &LiteralQuery,
        listing: Listing<'_>,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Vec<u8>> {
        let needle = listing.filter.map(str::to_lowercase);
        let stage = perf.start_stage();
        let found = self.search_matches(
            navigation,
            functions,
            query,
            needle.as_deref(),
            limits,
            perf,
        );
        perf.record("search", stage);
        let (found, immediate_records) = found?;
        let data = crate::data::DataInventory::discover(navigation, functions, limits, perf)?;
        let stage = perf.start_stage();
        let data_found = data.search(navigation, query, needle.as_deref(), limits, perf);
        perf.record("search", stage);
        let (data_found, data_records) = data_found?;
        let records = immediate_records.saturating_add(data_records);
        crate::data::DataInventory::check_records(records, limits)?;
        perf.counters.records = Some(records);
        perf.counters.matches = Some(found.len() + data_found.len());
        let stage = perf.start_stage();
        let result = (|| {
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                limit: limits.output_bytes,
                exceeded: false,
            };
            let mut header = self.header(
                navigation,
                "strings",
                query.json(navigation),
                listing.include_bytecode,
            );
            header["query"]["filter"] = json!(listing.filter);
            header["detail"].as_object_mut().unwrap().extend(object(
                serde_json::to_value(data.coverage(navigation, functions))
                    .expect("fixed data coverage schema"),
            ));
            write_record(&mut output, &header)?;
            for item in &found {
                for owner in Self::routes(functions, item.literal) {
                    write_record(
                        &mut output,
                        &self.record(
                            navigation,
                            functions,
                            item.literal,
                            owner,
                            Some(SearchEvidence {
                                query,
                                candidates: &item.candidates,
                                matches: &item.matches,
                            }),
                        ),
                    )?;
                }
            }
            for record in data.search_records(navigation, functions, query, &data_found) {
                write_record(&mut output, &record)?;
            }
            write_record(
                &mut output,
                &envelope::summary(
                    records,
                    true,
                    crate::navigation::summary_detail(object(json!({
                "literals": found.len() + data_found.len(), "data_literals": data_found.len(), "references": null, "reference_status": null, "excluded_unindexed_pushes": self.excluded_unindexed_pushes,
                "disassembly_complete": navigation.decoded_end == navigation.carrier.bytecode.len(), "discovery_complete": null}))),
                ),
            )?;
            Ok(output.bytes)
        })();
        perf.record("serialize", stage);
        result
    }

    pub fn refs_json(
        &self,
        navigation: &Navigation,
        functions: &FunctionNavigation,
        at: &str,
        listing: Listing<'_>,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Vec<u8>> {
        let needle = listing.filter.map(str::to_lowercase);
        let id = crate::canonical_id(&navigation.carrier.source_id, at);
        if id.split(':').nth(5) == Some("data") {
            let data = crate::data::DataInventory::discover(navigation, functions, limits, perf)?;
            let stage = perf.start_stage();
            let selected = data.refs(navigation, functions, &id, needle.as_deref(), limits, perf);
            perf.record("references", stage);
            let (literal, routes, status) = selected?;
            perf.counters.records = Some(routes.len());
            perf.counters.matches = Some(1);
            let stage = perf.start_stage();
            let result = (|| {
                let mut output = BoundedOutput {
                    bytes: Vec::new(),
                    limit: limits.output_bytes,
                    exceeded: false,
                };
                write_record(
                    &mut output,
                    &references::header(
                        navigation,
                        &id,
                        listing,
                        HeaderDetail {
                            target_kind: "literal",
                            target_decoded_offset: literal.start,
                            target_semantics: "bytecode_data_candidate",
                            coverage: "contextual_static_evidence",
                            literal_scope: None,
                            excluded_unindexed_pushes: Some(self.excluded_unindexed_pushes),
                            data: Some(data.coverage(navigation, functions)),
                        },
                    ),
                )?;
                for &route in &routes {
                    write_record(
                        &mut output,
                        &data.reference_record(navigation, functions, &literal, route),
                    )?;
                }
                write_record(
                    &mut output,
                    &references::summary(
                        navigation,
                        SummaryDetail {
                            target_kind: "literal",
                            target_decoded_offset: literal.start,
                            references: routes.len(),
                            literals: Some(1),
                            data_literals: Some(1),
                            reference_status: Some(status),
                            excluded_unindexed_pushes: Some(self.excluded_unindexed_pushes),
                            ..SummaryDetail::default()
                        },
                    ),
                )?;
                Ok(output.bytes)
            })();
            perf.record("serialize", stage);
            return result;
        }
        let stage = perf.start_stage();
        let selected = (|| {
            work(
                perf,
                id.len().saturating_add(self.literals.len()),
                limits.work,
            )?;
            let literal = self.select(navigation, &id)?;
            let count = functions.owners[literal.block].len().max(1);
            work(perf, count, limits.work)?;
            if count > limits.records {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "EVM result record budget exceeded",
                ));
            }
            Ok(literal)
        })();
        perf.record("references", stage);
        let literal = selected?;
        let routes = Self::routes(functions, literal);
        perf.counters.records = Some(routes.len());
        perf.counters.matches = Some(1);
        if routes.len() > limits.records {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "EVM result record budget exceeded",
            ));
        }
        let stage = perf.start_stage();
        let result = (|| {
            let mut output = BoundedOutput {
                bytes: Vec::new(),
                limit: limits.output_bytes,
                exceeded: false,
            };
            write_record(
                &mut output,
                &references::header(
                    navigation,
                    &id,
                    listing,
                    HeaderDetail {
                        target_kind: "literal",
                        target_decoded_offset: navigation.instructions[literal.instruction]
                            .decoded_offset,
                        target_semantics: "exact_literal_occurrence",
                        coverage: "validated_block_push_immediates",
                        data: None,
                        literal_scope: Some("validated_block_push_immediates"),
                        excluded_unindexed_pushes: Some(self.excluded_unindexed_pushes),
                    },
                ),
            )?;
            let mut emitted = 0;
            for owner in routes {
                let record = self.record(navigation, functions, literal, owner, None);
                if !references::keeps(&record, needle.as_deref()) {
                    continue;
                }
                write_record(&mut output, &record)?;
                emitted += 1;
            }
            write_record(
                &mut output,
                &references::summary(
                    navigation,
                    SummaryDetail {
                        target_kind: "literal",
                        target_decoded_offset: navigation.instructions[literal.instruction]
                            .decoded_offset,
                        references: emitted,
                        literals: Some(1),
                        data_literals: Some(0),
                        reference_status: Some("decoded_push_use"),
                        excluded_unindexed_pushes: Some(self.excluded_unindexed_pushes),
                        ..SummaryDetail::default()
                    },
                ),
            )?;
            Ok(output.bytes)
        })();
        perf.record("serialize", stage);
        result
    }
}
