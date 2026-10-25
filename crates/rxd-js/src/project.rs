//! Project references join lexical bindings through explicit relative ESM imports/re-exports.
//! Package resolution, namespace property inference and missing files remain unlinked.
use crate::{RefsTarget, SourceMode, Syntax, modules::Index, structure::Collector};
use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use rxd_core::{BudgetPolicy, Error, Limits, Result, Source, perf::Perf};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    path::{Component, Path, PathBuf},
};
struct File {
    path: PathBuf,
    bytes: usize,
    index: Index,
    limits: Limits,
}
type Binding = (usize, String);
enum Resolution {
    Found(Binding),
    Absent,
    Unknown,
}
impl Resolution {
    fn binding(self) -> Option<Binding> {
        if let Self::Found(binding) = self {
            Some(binding)
        } else {
            None
        }
    }
    fn merge(self, next: Self) -> Self {
        match (self, next) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Absent, other) | (other, Self::Absent) => other,
            (Self::Found(a), Self::Found(b)) if a == b => Self::Found(a),
            _ => Self::Unknown,
        }
    }
}

pub fn xref(
    paths: &[PathBuf],
    anchor: &Path,
    target: &RefsTarget,
    mode: SourceMode,
    syntax: Syntax,
    policy: BudgetPolicy,
    perf: &mut Perf,
) -> Result<Vec<u8>> {
    let mut files = Vec::new();
    for path in paths {
        let limits = Limits::from(policy.for_file(path)?);
        let mut shard = Perf::new(perf.enabled(), "analysis", "js", "xref");
        let source = Source::read(path, limits, &mut shard)?;
        let allocator = Allocator::default();
        let parsed = shard.measure("parse", || {
            Parser::new(
                &allocator,
                &source.text,
                crate::syntax::select(path, mode, syntax),
            )
            .with_options(ParseOptions {
                parse_regular_expression: true,
                ..Default::default()
            })
            .parse()
        });
        if parsed.fatal_error || !parsed.diagnostics.is_empty() {
            return Err(Error::new(
                "INVALID_INPUT",
                format!(
                    "{}: {}",
                    path.display(),
                    parsed
                        .diagnostics
                        .iter()
                        .take(3)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            ));
        }
        let mut collector = Collector::new(&source, limits, None, false, None, false, None);
        use oxc_ast_visit::Visit;
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
            collector.visit_program(&parsed.program)
        })) {
            return match payload.downcast::<Error>() {
                Ok(error) => Err(*error),
                Err(payload) => resume_unwind(payload),
            };
        }
        let index = Index::build(
            &parsed.program,
            &source,
            &collector,
            limits,
            &mut shard,
            true,
        )?;
        shard.counters.entities = Some(collector.entities.len());
        perf.merge(&shard);
        files.push(File {
            path: path.clone(),
            bytes: source.text.len(),
            index,
            limits,
        });
    }
    let anchor = files
        .iter()
        .position(|f| f.path == anchor)
        .ok_or_else(|| Error::new("NOT_FOUND", "selected file is not in the JS snapshot"))?;
    let selected = match target {
        RefsTarget::Coordinate(at) => {
            let parts: Vec<_> = at.split(':').collect();
            if parts.len() != 3 || !matches!(parts[0], "symbol" | "identifier") {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "project xref requires a symbol or identifier coordinate",
                ));
            }
            let start = parts[1]
                .parse::<u32>()
                .map_err(|_| Error::new("INVALID_QUERY", "invalid coordinate"))?;
            let end = parts[2]
                .parse::<u32>()
                .map_err(|_| Error::new("INVALID_QUERY", "invalid coordinate"))?;
            if at != &format!("{}:{start}:{end}", parts[0])
                || start >= end
                || end as usize > files[anchor].bytes
            {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "coordinate is outside the selected file",
                ));
            }
            if parts[0] == "symbol" {
                let binding = format!("symbol:{start}:{end}");
                if !binding_exists(&files[anchor].index, &binding) {
                    return Err(Error::new(
                        "NOT_FOUND",
                        "coordinate does not identify a source binding",
                    ));
                }
                binding
            } else {
                files[anchor]
                    .index
                    .references
                    .iter()
                    .find(|r| r.span.start == start && r.span.end == end)
                    .map(|r| r.binding_at.clone())
                    .or_else(|| {
                        files[anchor]
                            .index
                            .bindings
                            .iter()
                            .find(|b| b.span.start == start && b.span.end == end)
                            .map(|b| b.at.clone())
                    })
                    .ok_or_else(|| {
                        Error::new("NOT_FOUND", "coordinate is not a bound identifier")
                    })?
            }
        }
        RefsTarget::Byte(byte) => {
            if *byte >= files[anchor].bytes as u64 {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "byte offset is outside the selected file",
                ));
            }
            files[anchor]
                .index
                .references
                .iter()
                .find(|r| u64::from(r.span.start) <= *byte && *byte < u64::from(r.span.end))
                .map(|r| r.binding_at.clone())
                .or_else(|| {
                    files[anchor]
                        .index
                        .bindings
                        .iter()
                        .find(|b| u64::from(b.span.start) <= *byte && *byte < u64::from(b.span.end))
                        .map(|b| b.at.clone())
                })
                .ok_or_else(|| Error::new("NOT_FOUND", "offset is not a bound identifier"))?
        }
        RefsTarget::Filter { .. } => {
            return Err(Error::new(
                "INVALID_QUERY",
                "--file selects one coordinate; use directory --match without --file for name searches",
            ));
        }
    };
    let lookup: HashMap<_, _> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (normalize(&f.path), i))
        .collect();
    let graph = Graph {
        files: &files,
        lookup,
    };
    let selected = graph
        .binding(anchor, &selected)
        .unwrap_or((anchor, selected));
    let mut output = Vec::new();
    let mut count = 0;
    let mut canonical = HashMap::new();
    perf.counters.input_bytes = Some(files.iter().map(|f| f.bytes).sum());
    for (file_index, file) in files.iter().enumerate() {
        let mut shard = crate::Output {
            bytes: Vec::new(),
            limit: file.limits.output_bytes,
            limit_hit: false,
        };
        let mut opened = false;
        for reference in &file.index.references {
            let target = canonical
                .entry((file_index, reference.binding_at.as_str()))
                .or_insert_with(|| {
                    graph
                        .binding(file_index, &reference.binding_at)
                        .unwrap_or((file_index, reference.binding_at.clone()))
                });
            if *target != selected {
                continue;
            }
            if !opened {
                shard.json(&json!({"record":"file","detail":{"path":file.path.to_string_lossy(),"input_bytes":file.bytes}}))?;
                opened = true;
            }
            shard.json(&json!({"record":"reference","at":null,"source_id":null,"parent_at":reference.parent_at,"context_at":reference.context_at,"start":reference.span.start,"end":reference.span.end,"detail":{"target":selected.1,"target_path":files[selected.0].path.to_string_lossy(),"read":reference.read,"write":reference.write,"member_write":reference.member_write,"type_reference":reference.type_reference,"value_reference":reference.value_reference,"link":if file_index==selected.0{"local"}else{"esm"}}}))?;
            count += 1;
        }
        output
            .try_reserve(shard.bytes.len())
            .map_err(|_| Error::new("RESOURCE_LIMIT", "project output allocation"))?;
        output.extend_from_slice(&shard.bytes);
    }
    perf.source_id = None;
    perf.counters.records = Some(count);
    perf.counters.output_bytes = Some(output.len());
    Ok(output)
}
fn binding_exists(index: &Index, binding: &str) -> bool {
    index.bindings.iter().any(|b| b.at == binding)
}
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}
struct Graph<'a> {
    files: &'a [File],
    lookup: HashMap<PathBuf, usize>,
}
impl Graph<'_> {
    fn source(&self, file: usize, source: &crate::strings::JsString) -> Option<usize> {
        let text = source.to_utf8()?;
        if (!text.starts_with("./") && !text.starts_with("../"))
            || text.contains(['?', '#', '%', '\\', '\0'])
        {
            return None;
        }
        let path = normalize(&self.files[file].path.parent()?.join(text));
        self.lookup.get(&path).copied()
    }
    fn imported(&self, file: usize, binding: &str) -> Option<&crate::modules::Import> {
        self.files[file].index.imports.iter().find(|i| {
            i.module_at == self.files[file].index.modules[0].at
                && i.syntax == "esm"
                && i.binding_at.as_deref() == Some(binding)
        })
    }
    fn binding(&self, file: usize, binding: &str) -> Option<Binding> {
        let Some(imported) = self.imported(file, binding) else {
            return Some((file, binding.to_owned()));
        };
        let name = imported.imported.as_ref()?;
        if name.equals_str("*") {
            return None;
        }
        self.export(self.source(file, imported.source.as_ref()?)?, name)
            .binding()
    }
    fn export<'a>(&'a self, file: usize, name: &'a crate::strings::JsString) -> Resolution {
        let mut pending = vec![(file, name)];
        let mut seen = HashSet::new();
        let mut result = Resolution::Absent;
        while let Some((file, name)) = pending.pop() {
            if !seen.insert((file, name.units())) {
                continue;
            }
            let exports = || {
                self.files[file].index.exports.iter().filter(|e| {
                    e.module_at == self.files[file].index.modules[0].at
                        && matches!(e.syntax, "esm" | "reexport" | "star")
                })
            };
            let mut explicit = false;
            for export in exports().filter(|e| e.exported == *name) {
                explicit = true;
                let (source, imported) = if let Some(at) = &export.local_at {
                    if let Some(import) = self.imported(file, at) {
                        (import.source.as_ref(), import.imported.as_ref())
                    } else {
                        result = result.merge(Resolution::Found((file, at.clone())));
                        continue;
                    }
                } else {
                    (export.source.as_ref(), export.imported.as_ref())
                };
                if let (Some(target), Some(imported)) =
                    (source.and_then(|s| self.source(file, s)), imported)
                {
                    if imported.equals_str("*") {
                        return Resolution::Unknown;
                    }
                    pending.push((target, imported));
                } else {
                    return Resolution::Unknown;
                }
            }
            if explicit || name.equals_str("default") {
                continue;
            }
            for star in exports().filter(|e| e.syntax == "star" && e.exported.equals_str("*")) {
                let Some(target) = star.source.as_ref().and_then(|s| self.source(file, s)) else {
                    return Resolution::Unknown;
                };
                pending.push((target, name));
            }
        }
        result
    }
}
