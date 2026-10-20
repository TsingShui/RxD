//! Bounded lexical HTML catalog. No DOM construction or input execution.
use std::{
    cell::RefCell,
    collections::HashSet,
    convert::Infallible,
    fs,
    io::{self, Write},
    path::Path,
    rc::Rc,
};

use html5gum::{
    Emitter, ForwardingEmitter, Span, State, Tokenizer,
    emitters::callback::{CallbackEmitter, CallbackEvent},
};
use rxd_core::{
    Error, Limits, Result,
    envelope::{self, RecordMeta},
    perf::Perf,
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

type CallbackFn<'a> = Box<dyn FnMut(CallbackEvent<'_>, Span<usize>) -> Option<Infallible> + 'a>;

struct StateSwitch<'a> {
    emitter: CallbackEmitter<CallbackFn<'a>, Infallible, usize>,
    last_start: Rc<RefCell<Option<Vec<u8>>>>,
}
impl ForwardingEmitter for StateSwitch<'_> {
    type Token = Infallible;
    fn inner(&mut self) -> &mut impl Emitter<Token = Self::Token> {
        &mut self.emitter
    }
    fn emit_current_tag(&mut self) -> Option<State> {
        let _ = self.emitter.emit_current_tag();
        let name = self.last_start.borrow_mut().take()?;
        // html5gum 0.8.4's naive table spells `noframes` as `noframe`.
        // WHATWG in-head parsing uses the plural spelling; retain the other naive states.
        // Provenance: https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inhead
        let state = match name.as_slice() {
            b"title" | b"textarea" => Some(State::RcData),
            b"script" => Some(State::ScriptData),
            b"style" | b"iframe" | b"xmp" | b"noembed" | b"noscript" | b"noframes" => {
                Some(State::RawText)
            }
            b"plaintext" => Some(State::PlainText),
            _ => None,
        };
        // The upstream emitter resets the character-event start when switching states.
        if state.is_some() {
            self.emitter.init_string();
        }
        state
    }
}

#[derive(Clone, Serialize)]
struct Attr {
    name: String,
    value: String,
}
struct Pending {
    tag: String,
    start: usize,
    attrs: Vec<Attr>,
}
struct Node {
    kind: &'static str,
    start: usize,
    end: usize,
    parent: Option<usize>,
    depth: usize,
    detail: Detail,
}
enum Detail {
    Element {
        tag: String,
        attrs: Vec<Attr>,
        self_closing: bool,
        is_void: bool,
        end_tag: &'static str,
        content_start: usize,
        content_end: usize,
    },
    Value,
}
struct Catalog<'a> {
    text: &'a str,
    nodes: Vec<Node>,
    stack: Vec<usize>,
    pending: Option<Pending>,
    limits: Limits,
    error: Option<Error>,
    tokenizer_errors: usize,
    unmatched_end_tags: usize,
    missing_end_tags: usize,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new("INVALID_INPUT", message)
}
fn resource(message: &'static str) -> Error {
    Error::new("RESOURCE_LIMIT", message)
}
fn normalized(bytes: &[u8]) -> Result<String> {
    let mut value = std::str::from_utf8(bytes)
        .map_err(|_| invalid("invalid decoded HTML UTF-8"))?
        .to_owned();
    value.make_ascii_lowercase();
    Ok(value)
}
fn is_void(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}
impl Catalog<'_> {
    fn slice(&self, start: usize, end: usize) -> Result<&str> {
        self.text
            .get(start..end)
            .ok_or_else(|| invalid("invalid tokenizer source range"))
    }
    fn add(
        &mut self,
        kind: &'static str,
        start: usize,
        end: usize,
        detail: Detail,
    ) -> Result<usize> {
        self.slice(start, end)?;
        if self.nodes.len() >= self.limits.records {
            return Err(resource("HTML record budget exceeded"));
        }
        let parent = self.stack.last().copied();
        let depth = self.stack.len();
        let index = self.nodes.len();
        self.nodes.push(Node {
            kind,
            start,
            end,
            parent,
            depth,
            detail,
        });
        Ok(index)
    }
    fn close(
        &mut self,
        index: usize,
        content_end: usize,
        end: usize,
        status: &'static str,
    ) -> Result<()> {
        self.slice(content_end, end)?;
        let node = &mut self.nodes[index];
        if let Detail::Element {
            content_start,
            content_end: ce,
            end_tag,
            ..
        } = &mut node.detail
        {
            if content_end < *content_start || end < content_end {
                return Err(invalid("invalid HTML element range"));
            }
            *ce = content_end;
            *end_tag = status;
            node.end = end;
        }
        if status == "missing" {
            self.missing_end_tags += 1;
        }
        Ok(())
    }
    fn event(&mut self, event: CallbackEvent<'_>, span: Span<usize>) -> Result<()> {
        match event {
            CallbackEvent::OpenStartTag { name } => {
                self.slice(span.start, span.end)?;
                if self.pending.is_some() {
                    return Err(invalid("unfinished HTML start tag"));
                }
                self.pending = Some(Pending {
                    tag: normalized(name)?,
                    start: span.start,
                    attrs: Vec::new(),
                });
            }
            CallbackEvent::AttributeName { name } => {
                if let Some(pending) = &mut self.pending {
                    pending.attrs.push(Attr {
                        name: normalized(name)?,
                        value: String::new(),
                    });
                }
            }
            CallbackEvent::AttributeValue { value } => {
                if let Some(attr) = self
                    .pending
                    .as_mut()
                    .and_then(|pending| pending.attrs.last_mut())
                {
                    attr.value = std::str::from_utf8(value)
                        .map_err(|_| invalid("invalid decoded HTML attribute value"))?
                        .to_owned();
                }
            }
            CallbackEvent::CloseStartTag { self_closing } => {
                let pending = self
                    .pending
                    .take()
                    .ok_or_else(|| invalid("HTML start tag has no opening"))?;
                self.slice(pending.start, span.end)?;
                let void = is_void(&pending.tag);
                let index = self.add(
                    "element",
                    pending.start,
                    span.end,
                    Detail::Element {
                        tag: pending.tag,
                        attrs: pending.attrs,
                        self_closing,
                        is_void: void,
                        end_tag: if void { "not_applicable" } else { "missing" },
                        content_start: span.end,
                        content_end: span.end,
                    },
                )?;
                if !void {
                    if self.stack.len() >= self.limits.ast_depth {
                        return Err(resource("HTML nesting depth budget exceeded"));
                    }
                    self.stack.push(index);
                }
            }
            CallbackEvent::EndTag { name } => {
                self.slice(span.start, span.end)?;
                let tag = normalized(name)?;
                let matched = self.stack.iter().rposition(|&index| matches!(&self.nodes[index].detail, Detail::Element { tag: open, .. } if open == &tag));
                if let Some(position) = matched {
                    while self.stack.len() > position + 1 {
                        let index = self.stack.pop().unwrap();
                        self.close(index, span.start, span.start, "missing")?;
                    }
                    let index = self.stack.pop().unwrap();
                    self.close(index, span.start, span.end, "explicit")?;
                } else {
                    self.unmatched_end_tags += 1;
                }
            }
            CallbackEvent::String { .. } => {
                self.add("text", span.start, span.end, Detail::Value)?;
            }
            CallbackEvent::Comment { .. } => {
                self.add("comment", span.start, span.end, Detail::Value)?;
            }
            CallbackEvent::Doctype { .. } => {
                self.add("doctype", span.start, span.end, Detail::Value)?;
            }
            CallbackEvent::Error(_) => self.tokenizer_errors += 1,
        }
        Ok(())
    }
}

struct Output {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
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
        serde_json::to_writer(&mut *self, value).map_err(|error| {
            Error::new(
                if error.is_io() {
                    "RESOURCE_LIMIT"
                } else {
                    "FORMAT_ERROR"
                },
                error.to_string(),
            )
        })?;
        self.write_all(b"\n")
            .map_err(|_| resource("output byte budget exceeded"))
    }
}

/// Select from the full bounded lexical catalog without changing record identities.
#[derive(Default)]
pub struct ListQuery {
    pub kind: Option<String>,
    pub tag: Option<String>,
    pub attr: Option<String>,
    pub within: Option<String>,
    pub depth: Option<usize>,
    /// Case-insensitive substring over the element name or the text/comment content.
    pub contains: Option<String>,
}

/// Prepared artifact bytes and the single success receipt. Nothing is written here; the
/// caller commits the artifact and then streams the receipt to stdout.
pub struct ExtractResponse {
    pub bytes: Vec<u8>,
    pub receipt: Vec<u8>,
}

fn attribute_name_valid(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().any(|ch| {
            ch.is_control() || ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '/')
        })
}

/// Canonical content-bound record identity: `rxd1:html:<sha256>:<kind>:<start>:<end>`.
fn record_id(hash: &str, kind: &str, start: usize, end: usize) -> String {
    format!("rxd1:html:{hash}:{kind}:{start}:{end}")
}

fn canonical_id(id: &str) -> bool {
    let parts: Vec<_> = id.split(':').collect();
    parts.len() == 6
        && parts[0] == "rxd1"
        && parts[1] == "html"
        && parts[2].len() == 64
        && parts[2]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && matches!(parts[3], "element" | "text" | "comment" | "doctype")
        && parts[4..].iter().all(|number| {
            number
                .parse::<usize>()
                .is_ok_and(|value| value.to_string() == *number)
        })
}

fn validate_id(id: &str, hash: &str, ids: &[String]) -> Result<usize> {
    // A row prints the copy-only `kind:start:end`; identity lives in the header, so the
    // short form is expanded here instead of making a caller reassemble the hash.
    let expanded;
    let id = if id.split(':').count() == 3 {
        expanded = format!("rxd1:html:{hash}:{id}");
        expanded.as_str()
    } else {
        id
    };
    if !canonical_id(id) {
        return Err(Error::new("INVALID_ID", "malformed HTML record ID"));
    }
    if id.split(':').nth(2) != Some(hash) {
        return Err(Error::new(
            "SOURCE_MISMATCH",
            "HTML record ID belongs to different source content",
        ));
    }
    ids.iter()
        .position(|candidate| candidate == id)
        .ok_or_else(|| Error::new("INVALID_ID", "ID does not identify a real HTML record"))
}

fn parse_catalog<'a>(
    text: &'a str,
    hash: &str,
    limits: Limits,
    perf: &mut Perf,
) -> Result<(Catalog<'a>, Vec<String>)> {
    let mut catalog = Catalog {
        text,
        nodes: Vec::new(),
        stack: Vec::new(),
        pending: None,
        limits,
        error: None,
        tokenizer_errors: 0,
        unmatched_end_tags: 0,
        missing_end_tags: 0,
    };
    perf.measure("tokenize", || -> Result<()> {
        let last_start = Rc::new(RefCell::new(None));
        let observed = last_start.clone();
        let callback: CallbackFn<'_> = Box::new(|event, span| {
            if let CallbackEvent::OpenStartTag { name } = &event {
                *observed.borrow_mut() = Some(name.to_vec());
            }
            if catalog.error.is_none()
                && let Err(error) = catalog.event(event, span)
            {
                catalog.error = Some(error);
            }
            None
        });
        let mut emitter = CallbackEmitter::new(callback);
        emitter.naively_switch_states(false);
        Tokenizer::new_with_emitter(
            text,
            StateSwitch {
                emitter,
                last_start,
            },
        )
        .finish()
        .map_err(|error| invalid(format!("HTML reader failure: {error:?}")))?;
        if let Some(error) = catalog.error.take() {
            return Err(error);
        }
        if catalog.pending.is_some() {
            return Err(invalid("unfinished HTML start tag"));
        }
        while let Some(index) = catalog.stack.pop() {
            catalog.close(index, text.len(), text.len(), "missing")?;
        }
        Ok(())
    })?;
    let ids: Vec<String> = catalog
        .nodes
        .iter()
        .map(|node| record_id(hash, node.kind, node.start, node.end))
        .collect();
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(invalid("duplicate HTML record identity"));
    }
    Ok((catalog, ids))
}

fn read_source(input: &Path, limits: Limits, perf: &mut Perf) -> Result<(String, String)> {
    let bytes = perf.measure("read", || rxd_core::read_bytes(input, limits.input_bytes))?;
    perf.counters.input_bytes = Some(bytes.len());
    let hash = perf.measure("hash", || format!("{:x}", Sha256::digest(&bytes)));
    if perf.enabled() {
        perf.source_id = Some(hash.clone());
    }
    let text = perf.measure("utf8", || {
        String::from_utf8(bytes).map_err(|_| invalid("input must be valid UTF-8"))
    })?;
    Ok((text, hash))
}

pub fn list(input: &Path, query: ListQuery, limits: Limits, perf: &mut Perf) -> Result<Vec<u8>> {
    let ListQuery {
        kind,
        tag,
        attr,
        within,
        depth,
        contains,
    } = query;
    if kind
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "element" | "text" | "comment" | "doctype"))
    {
        return Err(Error::new("INVALID_QUERY", "invalid HTML record kind"));
    }
    let tag = tag.map(|mut tag| {
        tag.make_ascii_lowercase();
        tag
    });
    let attr = attr
        .map(|selector| {
            let (name, value) = selector
                .split_once('=')
                .map_or((selector.as_str(), None), |(name, value)| {
                    (name, Some(value))
                });
            if !attribute_name_valid(name) {
                return Err(Error::new("INVALID_QUERY", "invalid HTML attribute name"));
            }
            let mut name = name.to_owned();
            name.make_ascii_lowercase();
            Ok::<_, Error>((name, value.map(str::to_owned)))
        })
        .transpose()?;
    let query_attr = attr.as_ref().map(|(name, value)| match value {
        Some(value) => format!("{name}={value}"),
        None => name.clone(),
    });
    let path = input
        .to_str()
        .ok_or_else(|| invalid("HTML input path must be UTF-8"))?;
    let (text, hash) = read_source(input, limits, perf)?;
    let (catalog, ids) = parse_catalog(&text, &hash, limits, perf)?;
    let order = perf.measure("query", || -> Result<Vec<usize>> {
        let root = if let Some(id) = within.as_deref() {
            let index = validate_id(id, &hash, &ids)?;
            if catalog.nodes[index].kind != "element" {
                return Err(Error::new("INVALID_QUERY", "--within requires an element ID"));
            }
            Some(index)
        } else {
            None
        };
        let mut inside = vec![false; catalog.nodes.len()];
        let mut selected = Vec::new();
        for (index, node) in catalog.nodes.iter().enumerate() {
            inside[index] = root.is_none()
                || root == Some(index)
                || node.parent.is_some_and(|parent| inside[parent]);
            let relative_depth = root.map_or(node.depth, |root| node.depth.saturating_sub(catalog.nodes[root].depth));
            let contains_ok = contains.as_ref().is_none_or(|needle| {
                let needle = needle.to_lowercase();
                match &node.detail {
                    Detail::Element { tag, .. } => tag.to_lowercase().contains(&needle),
                    _ => node.kind.to_lowercase().contains(&needle)
                        || text
                            .get(node.start..node.end)
                            .is_some_and(|raw| raw.to_lowercase().contains(&needle)),
                }
            });
            let matches_attr = attr.as_ref().is_none_or(|(name, value)| {
                matches!(&node.detail, Detail::Element { attrs, .. } if attrs.iter().any(|a| a.name == *name && value.as_ref().is_none_or(|v| a.value == *v)))
            });
            let matches_tag = tag.as_ref().is_none_or(|expected| {
                matches!(&node.detail, Detail::Element { tag, .. } if tag == expected)
            });
            if inside[index]
                && depth.is_none_or(|limit| relative_depth <= limit)
                && kind.as_deref().is_none_or(|expected| node.kind == expected)
                && matches_tag
                && matches_attr
                && contains_ok
            {
                selected.push(index);
            }
        }
        selected.sort_by_key(|&index| {
            let node = &catalog.nodes[index];
            (node.start, node.kind, node.end)
        });
        Ok(selected)
    })?;
    let mut output = Output {
        bytes: Vec::new(),
        limit: limits.output_bytes,
    };
    perf.measure("serialize", || -> Result<()> {
        output.json(&envelope::header("html", "entries", Some(&hash), Some(text.len()),
            json!({"kind":kind,"tag":tag,"attr":query_attr,"depth":depth,"within":within,"contains":contains}),
            json!({"path":path,"analysis":"lexical","parser":"html5gum","parser_version":"0.8.4","lexical_profile":"lexical_naive_v1"}).as_object().unwrap().clone()))?;
        for &index in &order {
            let node = &catalog.nodes[index];
            let id = &ids[index];
            let parent = node.parent.and_then(|index| catalog.nodes.get(index)).map(|parent| record_id(&hash, parent.kind, parent.start, parent.end));
            let detail = match &node.detail {
                Detail::Element { tag, attrs, self_closing, is_void, end_tag, content_start, content_end } =>
                    json!({"tag":tag,"attrs":attrs,"self_closing":self_closing,"void":is_void,"depth":node.depth,"end_tag":end_tag,"content_start":content_start,"content_end":content_end}),
                Detail::Value => json!({"value":catalog.slice(node.start, node.end)?,"depth":node.depth}),
            };
            output.json(&envelope::record(node.kind, RecordMeta { at: Some(id), parent_at: parent.as_deref(), context_at: if node.kind == "element" { Some(id) } else { parent.as_deref() }, start: Some(node.start), end: Some(node.end), ..RecordMeta::default() }, detail.as_object().unwrap().clone()))?;
        }
        let count = |kind| catalog.nodes.iter().filter(|node| node.kind == kind).count();
        output.json(&envelope::summary(order.len(), true,
            json!({"nodes":catalog.nodes.len(),"elements":count("element"),"texts":count("text"),"comments":count("comment"),"doctypes":count("doctype"),"tokenizer_errors":catalog.tokenizer_errors,"unmatched_end_tags":catalog.unmatched_end_tags,"missing_end_tags":catalog.missing_end_tags}).as_object().unwrap().clone()))
    })?;
    perf.counters.records = Some(order.len());
    perf.counters.output_bytes = Some(output.bytes.len());
    Ok(output.bytes)
}

/// Fetch the exact original container interval selected by a validated catalog ID.
///
/// `--content` is element-only and selects the element's content range; default selection
/// covers the whole record range. All validation, byte selection, artifact identity and
/// receipt serialization happen here, before the caller commits the artifact.
pub fn extract(
    input: &Path,
    id: &str,
    content: bool,
    output: &Path,
    limits: Limits,
    perf: &mut Perf,
) -> Result<ExtractResponse> {
    let path = input
        .to_str()
        .ok_or_else(|| invalid("HTML input path must be UTF-8"))?;
    let destination_path = output
        .to_str()
        .ok_or_else(|| invalid("output path must be UTF-8"))?;
    let (text, hash) = read_source(input, limits, perf)?;
    let (catalog, ids) = parse_catalog(&text, &hash, limits, perf)?;
    let index = validate_id(id, &hash, &ids)?;
    let node = &catalog.nodes[index];
    let (start, end, part) = if content {
        match &node.detail {
            Detail::Element {
                content_start,
                content_end,
                ..
            } => (*content_start, *content_end, "content"),
            _ => {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "--content requires an element ID",
                ));
            }
        }
    } else {
        (node.start, node.end, "outer")
    };
    if content && start == end {
        return Err(Error::new("NOT_FOUND", "element has no content"));
    }
    // Never replace the container the selected bytes depend on, including path aliases.
    if let Ok(destination) = fs::canonicalize(output)
        && fs::canonicalize(input).is_ok_and(|source| source == destination)
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "output must not replace an input file",
        ));
    }
    let bytes = text.as_bytes()[start..end].to_vec();
    if bytes.len() > limits.output_bytes {
        return Err(resource("output byte budget exceeded"));
    }
    let source_id = perf.measure("hash", || format!("{:x}", Sha256::digest(&bytes)));
    if perf.enabled() {
        perf.source_id = Some(source_id.clone());
    }
    let mut receipt = Output {
        bytes: Vec::new(),
        limit: limits.output_bytes - bytes.len(),
    };
    perf.measure("serialize", || -> Result<()> {
        let mut receipt_record = envelope::record(
            "file",
            RecordMeta {
                source_id: Some(&source_id),
                start: Some(0),
                end: Some(bytes.len()),
                ..RecordMeta::default()
            },
            json!({
                "complete": true,
                "path": destination_path,
                "file_bytes": bytes.len(),
                "representation": "original",
                "part": part,
                "origin": {
                    "container": path,
                    "format": "html",
                    "source_id": hash,
                    "node_id": id,
                    "kind": node.kind,
                    "part": part,
                    "start": start,
                    "end": end,
                }
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        envelope::attach_protocol(&mut receipt_record, "html", "extract");
        receipt.json(&receipt_record)
    })?;
    perf.counters.records = Some(1);
    perf.counters.output_bytes = Some(bytes.len() + receipt.bytes.len());
    Ok(ExtractResponse {
        bytes,
        receipt: receipt.bytes,
    })
}
