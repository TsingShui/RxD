//! JavaScript parsing and structure queries. Input is never executed.

mod calls;
mod definition;
mod envelope;
mod fragment;
mod identifiers;
mod lexical;
mod members;
mod refs;
mod static_eval;
mod strings;
mod structure;
mod symbols;
mod syntax;

pub use syntax::Syntax;

use std::{
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    path::Path,
};

use oxc_allocator::Allocator;
use oxc_ast_visit::Visit;
use oxc_parser::{ParseOptions, Parser};
use rxd_core::envelope::{RecordMeta, header, summary};
use rxd_core::perf::Perf;
use rxd_core::{Error, Limits, Origin, Result, Source};
use serde::Serialize;
use serde_json::json;
use structure::Collector;

#[derive(Clone, Copy, Debug)]
pub enum SourceMode {
    Auto,
    Script,
    Module,
}

#[derive(Clone, Copy)]
pub enum IdentifierRole {
    All,
    Declaration,
    Reference,
}

/// How `xref` anchors on the name occurrence it resolves.
///
/// The two are not the same question. A coordinate is what a row prints and addresses the
/// occurrence's start; a byte offset may land *inside* a multi-byte identifier, which is what
/// a cursor position is. Both resolve to the same binding, so both stay.
#[derive(Clone, Debug)]
pub enum RefsTarget {
    Coordinate(String),
    Byte(u64),
}

#[derive(Clone)]
pub enum Query {
    Outline {
        /// `functions`, `classes` or `methods`; `None` is the full `entities` catalog.
        group: Option<&'static str>,
        within: Option<String>,
        depth: Option<usize>,
        kind: Option<String>,
        name: Option<String>,
        /// Substring over the entity name or context label, folded unless
        /// `contains_case_sensitive`.
        contains: Option<String>,
        /// Whether `contains` folds case; the CLI's `--case` decides it.
        contains_case_sensitive: bool,
    },
    Find {
        string: String,
        resolve_static: bool,
    },
    Identifiers {
        name: String,
        role: IdentifierRole,
        within: Option<String>,
    },
    Calls {
        name: String,
        within: Option<String>,
    },
    Members {
        name: String,
        within: Option<String>,
    },
    Locate {
        byte: u64,
    },
    Refs {
        /// Where the name occurrence is: the coordinate a row prints, or a raw byte offset.
        target: RefsTarget,
    },
    Symbols {
        within: Option<String>,
    },
    Get {
        id: String,
        json: bool,
    },
}

/// Kind sets behind the grouped listing commands.
///
/// `entities` prints every kind, so these three views only partition the catalog; they
/// never introduce a kind and never lose one from the full listing.
pub const FUNCTION_KINDS: &[&str] = &[
    "function_declaration",
    "function_expression",
    "arrow_function",
];
pub const CLASS_KINDS: &[&str] = &[
    "class_declaration",
    "class_expression",
    "interface",
    "enum",
    "type_alias",
    "namespace",
    "external_module",
];
pub const METHOD_KINDS: &[&str] = &[
    "class_method",
    "constructor",
    "getter",
    "setter",
    "object_method",
    "method_signature",
];

pub fn group_kinds(group: &str) -> Option<&'static [&'static str]> {
    match group {
        "functions" => Some(FUNCTION_KINDS),
        "classes" => Some(CLASS_KINDS),
        "methods" => Some(METHOD_KINDS),
        _ => None,
    }
}

impl Query {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Outline { group, .. } => group.unwrap_or("entities"),
            Self::Find { .. } => "find",
            Self::Identifiers { .. } => "identifiers",
            Self::Calls { .. } => "calls",
            Self::Members { .. } => "members",
            Self::Locate { .. } => "locate",
            Self::Refs { .. } => "xref",
            Self::Symbols { .. } => "symbols",
            Self::Get { .. } => "get",
        }
    }
}

/// Expand the copy-only `kind:start:end` a row prints into the content-bound id.
///
/// A caller copies `at` and passes it back; the schema prefix and the hash live in the
/// stream header, so the library reattaches them instead of asking the caller to carry
/// them. Anything already prefixed (or malformed) is passed through untouched and
/// validated by the normal path.
pub fn expand_short_id(source: &Source, id: &str) -> String {
    if id.starts_with("rxd1:") || id.split(':').count() != 3 {
        return id.to_owned();
    }
    format!("rxd1:js:{}:{id}", source.id)
}

/// The byte an occurrence coordinate starts at.
///
/// `xref` anchors on a *name occurrence*, so the accepted coordinates are the ones that
/// print one: a `symbols` row's declaration (`symbol:6:9`) or an occurrence position
/// (`identifier:26:31`). A structure coordinate names a node, not a name, and is rejected
/// here rather than silently resolving to whatever identifier happens to start at the same
/// byte.
fn occurrence_start(source: &Source, at: &str) -> Result<u64> {
    let expanded = expand_short_id(source, at);
    let parts: Vec<&str> = expanded.split(':').collect();
    if parts.len() != 6
        || parts[0] != "rxd1"
        || parts[1] != "js"
        || parts[2].len() != 64
        || !parts[2]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(Error::new(
            "INVALID_ID",
            format!("{at} is not a `<kind>:<start>:<end>` coordinate"),
        ));
    }
    if parts[2] != source.id {
        return Err(Error::new(
            "SOURCE_MISMATCH",
            "coordinate belongs to different source content",
        ));
    }
    let kind = parts[3];
    if kind != "symbol" && kind != "identifier" {
        return Err(Error::new(
            "INVALID_ID",
            format!(
                "{at} is a {kind} node, not a name occurrence; xref anchors on a `symbols` row \
                 (`symbol:6:9`) or an occurrence position (`identifier:26:31`)"
            ),
        ));
    }
    let start = parts[4]
        .parse::<u64>()
        .map_err(|_| Error::new("INVALID_ID", format!("{at} has no byte offset")))?;
    Ok(start)
}

/// Serialize into a bounded buffer before exposing any query result.
struct Output {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("output byte budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Output {
    fn json(&mut self, value: &impl Serialize) -> Result<()> {
        if let Err(error) = serde_json::to_writer(&mut *self, value) {
            return Err(if self.exceeded {
                Error::new("RESOURCE_LIMIT", "output byte budget exceeded")
            } else {
                Error::new("FORMAT_ERROR", error.to_string())
            });
        }
        self.raw(b"\n")
    }
    fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        self.write_all(bytes)
            .map_err(|_| Error::new("RESOURCE_LIMIT", "output byte budget exceeded"))
    }
}

#[derive(Serialize)]
struct SourceDetail<'a, E, S, F> {
    complete: bool,
    representation: &'static str,
    source: &'a str,
    entity: Option<E>,
    symbol: Option<S>,
    declaration: Option<&'a definition::Declaration>,
    fragment: Option<F>,
    syntax: Option<&'static str>,
    declaration_file: Option<bool>,
    origin: Option<&'a Origin>,
}

pub fn run(
    path: &Path,
    mode: SourceMode,
    syntax: Syntax,
    query: Query,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<u8>> {
    if let Query::Find { string, .. } = &query
        && string.is_empty()
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "search string must not be empty",
        ));
    }
    let source = Source::read(path, limits, perf)?;
    run_source(source, mode, syntax, query, limits, perf)
}

/// Reuse JS analysis for a container entry without extraction or a second read.
pub fn run_source(
    source: Source,
    mode: SourceMode,
    syntax: Syntax,
    query: Query,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<u8>> {
    if matches!(&query, Query::Find { string, .. } if string.is_empty())
        || matches!(&query, Query::Identifiers { name, .. } if name.is_empty())
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "search string must not be empty",
        ));
    }
    let parse_start = perf.start_stage();
    let source_type = syntax::select(Path::new(&source.path), mode, syntax);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source.text, source_type)
        .with_options(ParseOptions {
            parse_regular_expression: true,
            ..ParseOptions::default()
        })
        .parse();
    let syntax_info = syntax::Info::new(parsed.program.source_type);
    perf.record("parse", parse_start);
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        let message = parsed
            .diagnostics
            .iter()
            .take(3)
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::new("INVALID_INPUT", message));
    }
    let visit_start = perf.start_stage();
    let query_string = if let Query::Find { string, .. } = &query {
        Some(string.clone())
    } else {
        None
    };
    // Echo the caller's spelling: a short coordinate stays short in `query` even though the
    // library expands it for lookup.
    let query_json = match &query {
        Query::Outline {
            group,
            within,
            depth,
            kind,
            name,
            contains,
            contains_case_sensitive,
        } => {
            json!({"group":group,"within":within,"depth":depth,"kind":kind,"name":name,"contains":contains,"case":if *contains_case_sensitive {"sensitive"} else {"insensitive"}})
        }
        Query::Find {
            string,
            resolve_static,
        } => json!({"string":string,"resolve_static":resolve_static}),
        Query::Identifiers { name, role, within } => json!({
            "name": name,
            "within": within,
            "role": match role {
                IdentifierRole::All => "all",
                IdentifierRole::Declaration => "declaration",
                IdentifierRole::Reference => "reference",
            }
        }),
        Query::Calls { name, within } => json!({"name":name,"within":within}),
        Query::Members { name, within } => json!({"name":name,"within":within}),
        Query::Refs { target } => match target {
            RefsTarget::Coordinate(at) => json!({"at":at}),
            RefsTarget::Byte(byte) => json!({"byte":byte}),
        },
        Query::Locate { byte } => json!({"byte":byte}),
        Query::Symbols { within } => json!({"within":within}),
        Query::Get { id, json } => json!({"id":id,"json":json}),
    };
    // Every id-shaped selector may arrive as the copy-only short coordinate a row prints
    // (`kind:start:end`); expand it against the header identity exactly once here so the
    // fragment, symbol and structure paths all see the content-bound form.
    let query = match query {
        Query::Get { id, json } => Query::Get {
            id: expand_short_id(&source, &id),
            json,
        },
        Query::Outline {
            within: Some(id),
            group,
            depth,
            kind,
            name,
            contains,
            contains_case_sensitive,
        } => Query::Outline {
            group,
            within: Some(expand_short_id(&source, &id)),
            depth,
            kind,
            name,
            contains,
            contains_case_sensitive,
        },
        Query::Identifiers {
            name,
            role,
            within: Some(id),
        } => Query::Identifiers {
            name,
            role,
            within: Some(expand_short_id(&source, &id)),
        },
        Query::Calls {
            name,
            within: Some(id),
        } => Query::Calls {
            name,
            within: Some(expand_short_id(&source, &id)),
        },
        Query::Members {
            name,
            within: Some(id),
        } => Query::Members {
            name,
            within: Some(expand_short_id(&source, &id)),
        },
        Query::Symbols { within: Some(id) } => Query::Symbols {
            within: Some(expand_short_id(&source, &id)),
        },
        other => other,
    };
    let contexts = match &query {
        Query::Find { .. } | Query::Identifiers { .. } => {
            Some(fragment::Contexts::new(fragment::Target::Matches))
        }
        Query::Locate { byte } => Some(fragment::Contexts::new(fragment::Target::Byte(*byte))),
        Query::Get { id, .. }
            if id
                .split(':')
                .nth(3)
                .is_some_and(|kind| kind.starts_with("fragment.")) =>
        {
            Some(fragment::Contexts::new(fragment::Target::Id(id.clone())))
        }
        _ => None,
    };
    let resolve_static = matches!(
        query,
        Query::Find {
            resolve_static: true,
            ..
        }
    );
    let mut collector = Collector::new(
        &source,
        limits,
        query_string.as_deref(),
        contexts,
        resolve_static,
    );
    let visited = catch_unwind(AssertUnwindSafe(|| {
        collector.visit_program(&parsed.program)
    }));
    perf.record("visit", visit_start);
    perf.counters.entities = Some(collector.entities.len());
    perf.counters.matches = Some(collector.matches.len());
    if let Err(payload) = visited {
        match payload.downcast::<Error>() {
            Ok(error) => return Err(*error),
            Err(payload) => resume_unwind(payload),
        }
    }
    let query_start = perf.start_stage();
    let root = &collector.entities[0].entity;
    let final_mode = if parsed.program.source_type.is_module() {
        "module"
    } else {
        "script"
    };
    let mut output = Output {
        bytes: Vec::new(),
        limit: limits.output_bytes,
        exceeded: false,
    };
    let mut header = header("js", query.name(), Some(&source.id), Some(source.text.len()), query_json,
        json!({"path":source.path,"root_id":envelope::short(&root.id),"source_type":final_mode,
            "analysis":null,"syntax":syntax_info.syntax,"declaration_file":syntax_info.declaration_file,"origin":source.origin})
        .as_object().unwrap().clone());
    // An id a caller can copy out of `detail` must be the same short coordinate a row
    // prints, so it can be pasted straight back into `--at`.
    if let Some(detail) = header.get_mut("detail") {
        envelope::shorten_detail_ids(detail);
    }
    match query {
        Query::Outline {
            group,
            within,
            depth,
            kind,
            name,
            contains,
            contains_case_sensitive,
        } => {
            let group_kinds = match group {
                None => None,
                Some(group) => Some(
                    group_kinds(group)
                        .ok_or_else(|| Error::new("INVALID_QUERY", "unknown JS listing group"))?,
                ),
            };
            let selection = (|| -> Result<Vec<usize>> {
                let selected_root = within
                    .as_deref()
                    .map(|id| collector.by_id(id))
                    .transpose()?
                    .unwrap_or(0);
                let root_depth = collector.entities[selected_root].entity.depth;
                // `--kind` accepts a comma-separated list of exact kinds. Every entry must be
                // non-empty, so a typo cannot silently widen or narrow the selection.
                let kinds: Option<Vec<&str>> = match kind.as_deref() {
                    None => None,
                    Some(value) => {
                        let parts: Vec<&str> = value.split(',').collect();
                        if parts.iter().any(|part| part.is_empty()) {
                            return Err(Error::new(
                                "INVALID_QUERY",
                                "kind must be a comma-separated list of non-empty kinds",
                            ));
                        }
                        Some(parts)
                    }
                };
                let indices: Vec<_> = collector
                    .ordered_entities()
                    .into_iter()
                    .filter(|&i| {
                        let entity = &collector.entities[i].entity;
                        let belongs = if within.is_some() {
                            collector.is_descendant(i, selected_root)
                        } else {
                            true
                        };
                        let depth_ok =
                            depth.is_none_or(|d| entity.depth.saturating_sub(root_depth) <= d);
                        let kind_ok = kinds
                            .as_ref()
                            .is_none_or(|kinds| kinds.contains(&entity.kind));
                        let group_ok = group_kinds.is_none_or(|kinds| kinds.contains(&entity.kind));
                        let contains_ok = contains.as_ref().is_none_or(|needle| {
                            entity.name.as_ref().is_some_and(|value| {
                                value.contains_filter(needle, contains_case_sensitive)
                            }) || entity.label.as_ref().is_some_and(|value| {
                                value.contains_filter(needle, contains_case_sensitive)
                            })
                        });
                        let name_ok = name.as_deref().is_none_or(|n| {
                            entity
                                .name
                                .as_ref()
                                .is_some_and(|value| value.equals_str(n))
                                || entity
                                    .label
                                    .as_ref()
                                    .is_some_and(|value| value.equals_str(n))
                        });
                        group_ok && belongs && depth_ok && kind_ok && name_ok && contains_ok
                    })
                    .collect();
                if indices.len() > limits.records {
                    return Err(Error::new(
                        "RESOURCE_LIMIT",
                        "output record budget exceeded",
                    ));
                }
                Ok(indices)
            })();
            perf.record("query", query_start);
            let indices = selection?;
            perf.counters.records = Some(indices.len());
            perf.measure("serialize", || -> Result<()> {
                output.json(&header)?;
                for &i in &indices {
                    output.json(&collector.entities[i].entity.envelope(&source.id))?;
                }
                output.json(&summary(indices.len(), true, Default::default()))?;
                Ok(())
            })?;
        }
        Query::Find { .. } => {
            collector.matches.sort_by_key(|m| (m.start, m.end));
            perf.record("query", query_start);
            perf.counters.records = Some(collector.matches.len());
            perf.measure("serialize", || -> Result<()> {
                output.json(&header)?;
                for found in &collector.matches {
                    output.json(&found.envelope(&source.id))?;
                }
                output.json(&summary(collector.matches.len(), true, Default::default()))?;
                Ok(())
            })?;
        }
        Query::Identifiers { name, role, within } => {
            perf.record("query", query_start);
            let result = identifiers::run(
                &parsed.program,
                &source,
                &collector,
                &name,
                role,
                within.as_deref(),
                limits,
                perf,
            )?;
            perf.counters.records = Some(result.len());
            perf.measure("serialize", || -> Result<()> {
                output.json(&header)?;
                for identifier in &result {
                    output.json(&identifier.envelope(&source.id))?;
                }
                output.json(&summary(result.len(), true, Default::default()))
            })?;
        }
        Query::Calls { name, within } => {
            perf.record("query", query_start);
            let result = calls::run(
                &parsed.program,
                &source,
                &collector,
                &name,
                within.as_deref(),
                limits,
                perf,
            )?;
            perf.counters.records = Some(result.len());
            perf.measure("serialize", || -> Result<()> {
                output.json(&header)?;
                for call in &result {
                    output.json(&call.envelope(&source.id))?;
                }
                output.json(&summary(result.len(), true, Default::default()))
            })?;
        }
        Query::Members { name, within } => {
            perf.record("query", query_start);
            let result = members::run(
                &parsed.program,
                &source,
                &collector,
                &name,
                within.as_deref(),
                limits,
                perf,
            )?;
            perf.counters.records = Some(result.len());
            perf.measure("serialize", || -> Result<()> {
                output.json(&header)?;
                for member in &result {
                    output.json(&member.envelope(&source.id))?;
                }
                output.json(&summary(result.len(), true, Default::default()))
            })?;
        }
        Query::Locate { byte } => {
            if byte >= source.text.len() as u64 {
                perf.record("query", query_start);
                return Err(Error::new(
                    "INVALID_OFFSET",
                    "byte offset must be smaller than file length",
                ));
            }
            let target = collector
                .entities
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    u64::from(e.entity.start) <= byte && byte < u64::from(e.entity.end)
                })
                .max_by_key(|(_, e)| e.entity.depth)
                .map(|(i, _)| i)
                .expect("program contains valid offsets");
            let mut ancestors = Vec::new();
            let context_id = collector
                .contexts
                .as_ref()
                .and_then(|contexts| contexts.selected(&collector, &source))
                .map(|context| context.id);
            let mut parent = collector.entities[target].parent;
            while let Some(i) = parent {
                ancestors.push(collector.entities[i].entity.envelope(&source.id));
                parent = collector.entities[i].parent;
            }
            perf.record("query", query_start);
            perf.counters.records = Some(1);
            #[derive(Serialize)]
            struct Located<E, A> {
                complete: bool,
                entity: E,
                ancestors: Vec<A>,
                syntax: Option<&'static str>,
                declaration_file: Option<bool>,
                origin: Option<Origin>,
            }
            let entity = &collector.entities[target].entity;
            let located = envelope::body(
                "locate",
                RecordMeta {
                    at: Some(&entity.id),
                    parent_at: entity.parent_id.as_deref(),
                    context_at: context_id.as_deref(),
                    start: Some(entity.start as usize),
                    end: Some(entity.end as usize),
                    ..RecordMeta::default()
                },
                Located {
                    complete: true,
                    entity: entity.envelope(&source.id),
                    ancestors,
                    syntax: syntax_info.syntax,
                    declaration_file: syntax_info.declaration_file,
                    origin: source.origin.clone(),
                },
            )
            .single("locate");
            perf.measure("serialize", || output.json(&located))?;
        }
        Query::Symbols { within } => {
            perf.record("query", query_start);
            let result = symbols::run(
                &parsed.program,
                &source,
                &collector,
                within.as_deref(),
                limits,
                perf,
            )?;
            let mut header = header;
            header["detail"]["analysis"] = json!("lexical");
            header["detail"]["resolution"] = json!(result.resolution);
            header["detail"]["within_id"] = json!(envelope::short(&result.within_id));
            let records = result.records.len();
            perf.counters.records = Some(records);
            perf.measure("serialize", || -> Result<()> {
                output.json(&header)?;
                for record in &result.records {
                    output.json(&record.envelope(&source.id))?;
                }
                output.json(&summary(records, true, Default::default()))
            })?;
        }
        Query::Refs { target } => {
            perf.record("query", query_start);
            // A row's coordinate is `<kind>:<start>:<end>`; the name occurrence starts at
            // `start`, and the kind stays in the coordinate so a nested node with the same
            // start cannot be confused for the anchor.
            let byte = match target {
                RefsTarget::Coordinate(at) => occurrence_start(&source, at.as_str())?,
                RefsTarget::Byte(byte) => byte,
            };
            let result = refs::run(&parsed.program, &source, &collector, byte, limits, perf)?;
            let mut header = header;
            header["detail"]["analysis"] = json!("lexical");
            header["detail"]["resolution"] = json!(result.resolution);
            #[derive(Serialize)]
            struct RefHeader<'a> {
                selected: &'a refs::Selected,
                declaration: &'a Option<lexical::Location>,
                declaration_ids: &'a Option<Vec<String>>,
            }
            let detail = envelope::ExtendedDetail {
                base: header["detail"].as_object().unwrap().clone(),
                extra: RefHeader {
                    selected: &result.selected,
                    declaration: &result.declaration,
                    declaration_ids: &result.declaration_ids,
                },
            };
            let header = envelope::Body {
                outer: header,
                detail,
            };
            let records = result.references.len();
            perf.counters.records = Some(records);
            perf.measure("serialize", || -> Result<()> {
                output.json(&header)?;
                for reference in &result.references {
                    output.json(&reference.envelope(&source.id))?;
                }
                output.json(&summary(records, true, Default::default()))
            })?;
        }
        Query::Get { id, json } => {
            if let Some(contexts) = &collector.contexts {
                let selected = (|| {
                    fragment::validate_id(&source, &id)?;
                    contexts
                        .selected(&collector, &source)
                        .filter(|fragment| fragment.id == id)
                        .ok_or_else(|| {
                            Error::new("INVALID_ID", "ID does not identify a real source fragment")
                        })
                })();
                perf.record("query", query_start);
                let fragment = selected?;
                let original = &source.text[fragment.start as usize..fragment.end as usize];
                perf.counters.records = Some(1);
                if json {
                    #[derive(Serialize)]
                    struct FragmentDetail<'a> {
                        kind: &'a str,
                        entity_id: &'a str,
                    }
                    let fragment_record = envelope::body(
                        "fragment",
                        RecordMeta {
                            at: Some(&fragment.id),
                            parent_at: fragment.parent_id.as_deref(),
                            start: Some(fragment.start as usize),
                            end: Some(fragment.end as usize),
                            ..Default::default()
                        },
                        FragmentDetail {
                            kind: fragment.kind,
                            entity_id: &fragment.entity_id,
                        },
                    );
                    let extracted = envelope::body(
                        "source",
                        RecordMeta {
                            at: Some(&fragment.id),
                            parent_at: fragment.parent_id.as_deref(),
                            start: Some(fragment.start as usize),
                            end: Some(fragment.end as usize),
                            ..Default::default()
                        },
                        SourceDetail {
                            complete: true,
                            representation: "original",
                            source: original,
                            entity: None::<()>,
                            symbol: None::<()>,
                            declaration: None,
                            fragment: Some(fragment_record),
                            syntax: syntax_info.syntax,
                            declaration_file: syntax_info.declaration_file,
                            origin: source.origin.as_ref(),
                        },
                    )
                    .single("get");
                    perf.measure("serialize", || output.json(&extracted))?;
                } else {
                    perf.measure("serialize", || output.raw(original.as_bytes()))?;
                }
                perf.counters.output_bytes = Some(output.bytes.len());
                return Ok(output.bytes);
            }
            if id.split(':').nth(3) == Some("symbol") {
                perf.record("query", query_start);
                let definition =
                    definition::run(&parsed.program, &source, &collector, &id, limits, perf)?;
                let range = &definition.declaration.range;
                let original = &source.text[range.start as usize..range.end as usize];
                perf.counters.records = Some(1);
                if json {
                    let symbol = definition.symbol.envelope(&source.id);
                    let extracted = envelope::body(
                        "source",
                        RecordMeta {
                            at: Some(&id),
                            parent_at: Some(&definition.symbol.location.entity_id),
                            start: Some(range.start as usize),
                            end: Some(range.end as usize),
                            ..Default::default()
                        },
                        SourceDetail {
                            complete: true,
                            representation: "original",
                            source: original,
                            entity: None::<()>,
                            symbol: Some(symbol),
                            declaration: Some(&definition.declaration),
                            fragment: None::<()>,
                            syntax: syntax_info.syntax,
                            declaration_file: syntax_info.declaration_file,
                            origin: source.origin.as_ref(),
                        },
                    )
                    .single("get");
                    perf.measure("serialize", || output.json(&extracted))?;
                } else {
                    perf.measure("serialize", || output.raw(original.as_bytes()))?;
                }
                perf.counters.output_bytes = Some(output.bytes.len());
                return Ok(output.bytes);
            }
            let selected = collector.by_id(&id);
            perf.record("query", query_start);
            let target = &collector.entities[selected?];
            let original = &source.text[target.entity.start as usize..target.entity.end as usize];
            perf.counters.records = Some(1);
            if json {
                let extracted = envelope::body(
                    "source",
                    RecordMeta {
                        at: Some(&target.entity.id),
                        parent_at: target.entity.parent_id.as_deref(),
                        start: Some(target.entity.start as usize),
                        end: Some(target.entity.end as usize),
                        ..Default::default()
                    },
                    SourceDetail {
                        complete: true,
                        representation: "original",
                        source: original,
                        entity: Some(target.entity.envelope(&source.id)),
                        symbol: None::<()>,
                        declaration: None,
                        fragment: None::<()>,
                        syntax: syntax_info.syntax,
                        declaration_file: syntax_info.declaration_file,
                        origin: source.origin.as_ref(),
                    },
                )
                .single("get");
                perf.measure("serialize", || output.json(&extracted))?;
            } else {
                perf.measure("serialize", || output.raw(original.as_bytes()))?;
            }
        }
    }
    perf.counters.output_bytes = Some(output.bytes.len());
    Ok(output.bytes)
}
