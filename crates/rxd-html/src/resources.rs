//! Source-backed resource candidates, not a DOM, browser load plan or network client.

use std::path::Path;

use rxd_core::{Error, Limits, Result, envelope, envelope::RecordMeta, perf::Perf};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::{ParseError, Url};

use super::{Attr, Catalog, Detail, Output, parse_catalog, read_source};

pub const RESOURCE_KINDS: &[&str] = &[
    "script",
    "style",
    "importmap",
    "data",
    "image",
    "font",
    "media",
    "document",
    "hint",
    "other",
];

#[derive(Default)]
pub struct ResourcesQuery {
    /// The captured document's final HTTP(S) URL, not the local artifact path.
    pub base_url: Option<String>,
    pub kind: Option<String>,
    /// `inline` bodies or `external` attribute references (not necessarily network URLs).
    pub location: Option<String>,
    pub filter: Option<String>,
    pub case_sensitive: bool,
    /// Include original inline bytes as JSON text, with their independent source identity.
    pub include_content: bool,
}

fn attr<'a>(attrs: &'a [Attr], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| attr.name == name)
        .map(|attr| attr.value.as_str())
}

fn ascii_trim(text: &str) -> &str {
    text.trim_matches(|c| matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))
}

fn javascript_type(value: &str) -> bool {
    [
        "application/ecmascript",
        "application/javascript",
        "application/x-ecmascript",
        "application/x-javascript",
        "text/ecmascript",
        "text/javascript",
        "text/javascript1.0",
        "text/javascript1.1",
        "text/javascript1.2",
        "text/javascript1.3",
        "text/javascript1.4",
        "text/javascript1.5",
        "text/jscript",
        "text/livescript",
        "text/x-ecmascript",
        "text/x-javascript",
    ]
    .iter()
    .any(|mime| value.eq_ignore_ascii_case(mime))
}

/// HTML script type classification only; this does not prepare or execute a script.
fn script_type(attrs: &[Attr]) -> &'static str {
    let explicit = attr(attrs, "type");
    let language;
    let value = match explicit {
        Some("") => "text/javascript",
        Some(value) => value,
        None => match attr(attrs, "language") {
            None | Some("") => "text/javascript",
            Some(value) => {
                language = format!("text/{value}");
                &language
            }
        },
    };
    // Only the JavaScript MIME test strips ASCII whitespace. Parameters are not essence
    // matches; `module ` and `text/javascript; charset=utf-8` are data blocks, not JS.
    if javascript_type(value) || explicit.is_some_and(|value| javascript_type(ascii_trim(value))) {
        "classic"
    } else if value.eq_ignore_ascii_case("module") {
        "module"
    } else if value.eq_ignore_ascii_case("importmap") {
        "importmap"
    } else if value.eq_ignore_ascii_case("speculationrules") {
        "speculationrules"
    } else {
        "data"
    }
}

struct Base<'a> {
    document: Option<Url>,
    url: Option<Url>,
    href: Option<&'a str>,
    node: Option<usize>,
    status: &'static str,
}

fn resolve_url(value: &str, base: Option<&Url>) -> (Option<String>, &'static str) {
    if ascii_trim(value).is_empty() {
        return (None, "empty");
    }
    match Url::parse(value) {
        Ok(url) => (Some(url.into()), "absolute"),
        Err(ParseError::RelativeUrlWithoutBase) => match base {
            Some(base) => match base.join(value) {
                Ok(url) => (Some(url.into()), "base"),
                Err(_) => (None, "invalid"),
            },
            None => (None, "unresolved"),
        },
        Err(_) => (None, "invalid"),
    }
}

fn document_base<'a>(
    catalog: &'a Catalog<'_>,
    template: &[bool],
    document: Option<Url>,
) -> Base<'a> {
    let mut base = Base {
        url: document.clone(),
        document,
        href: None,
        node: None,
        status: "document",
    };
    for (index, node) in catalog.nodes.iter().enumerate() {
        let Detail::Element { tag, attrs, .. } = &node.detail else {
            continue;
        };
        if tag != "base" || template[index] {
            continue;
        }
        let Some(href) = attr(attrs, "href") else {
            continue;
        };
        base.href = Some(href);
        base.node = Some(index);
        let parsed = Url::parse(href).or_else(|error| {
            if error == ParseError::RelativeUrlWithoutBase {
                base.document
                    .as_ref()
                    .map_or(Err(error), |document| document.join(href))
            } else {
                Err(error)
            }
        });
        match parsed {
            Ok(url) if !matches!(url.scheme(), "data" | "javascript") => {
                base.url = Some(url);
                base.status = "element";
            }
            Ok(_) => base.status = "disallowed_scheme_fallback",
            Err(ParseError::RelativeUrlWithoutBase) if base.document.is_none() => {
                base.status = "unresolved"
            }
            Err(_) => base.status = "invalid_fallback",
        }
        // The first href wins, even if it is invalid. Do not silently try a later base.
        return base;
    }
    if base.document.is_none() {
        base.status = "missing";
    }
    base
}

struct Candidate<'a> {
    node: usize,
    kind: &'static str,
    relation: &'static str,
    attribute: Option<&'static str>,
    name: &'a str,
    script_type: Option<&'static str>,
    unresolved: Option<&'static str>,
}

impl Candidate<'_> {
    fn location(&self) -> &'static str {
        if self.attribute.is_some() {
            "external"
        } else {
            "inline"
        }
    }
}

fn link_kind(rel: &str, attrs: &[Attr]) -> Option<(&'static str, &'static str)> {
    match rel {
        "stylesheet" => Some(("style", "stylesheet")),
        "modulepreload" => Some(("script", "modulepreload")),
        "preconnect" => Some(("hint", "preconnect")),
        "dns-prefetch" => Some(("hint", "dns-prefetch")),
        "icon" => Some(("image", "icon")),
        "manifest" => Some(("data", "manifest")),
        "preload" => {
            let kind = match attr(attrs, "as")
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "script" => "script",
                "style" => "style",
                "image" => "image",
                "font" => "font",
                "audio" | "video" | "track" => "media",
                "document" => "document",
                "fetch" => "data",
                _ => "other",
            };
            Some((kind, "preload"))
        }
        _ => None,
    }
}

fn candidates<'a>(catalog: &'a Catalog<'_>) -> (Vec<Candidate<'a>>, Vec<bool>, usize) {
    let mut result = Vec::new();
    let mut template = vec![false; catalog.nodes.len()];
    let mut empty_inline = 0;
    for (index, node) in catalog.nodes.iter().enumerate() {
        template[index] = node.parent.is_some_and(|parent| template[parent]);
        let Detail::Element {
            tag,
            attrs,
            content_start,
            content_end,
            ..
        } = &node.detail
        else {
            continue;
        };
        template[index] |= tag == "template";
        let mut push =
            |kind, relation, attribute: Option<&'static str>, script_type, unresolved| {
                let name = if let Some(attribute) = attribute {
                    let Some(value) = attr(attrs, attribute) else {
                        return;
                    };
                    value
                } else {
                    if content_start == content_end {
                        empty_inline += 1;
                        return;
                    }
                    tag.as_str()
                };
                result.push(Candidate {
                    node: index,
                    kind,
                    relation,
                    attribute,
                    name,
                    script_type,
                    unresolved,
                });
            };
        match tag.as_str() {
            "script" => {
                let script = script_type(attrs);
                let kind = match script {
                    "classic" | "module" => "script",
                    "importmap" => "importmap",
                    _ => "data",
                };
                // A data block ignores src; external import maps/speculation rules are
                // unsupported, so preserve their source evidence without a fetch URL.
                let external = script != "data" && attr(attrs, "src").is_some();
                push(
                    kind,
                    "script",
                    external.then_some("src"),
                    Some(script),
                    (external && matches!(script, "importmap" | "speculationrules"))
                        .then_some("unsupported_script_src"),
                );
            }
            "style" => {
                let css = attr(attrs, "type")
                    .is_none_or(|value| value.is_empty() || value.eq_ignore_ascii_case("text/css"));
                push(
                    if css { "style" } else { "data" },
                    "style",
                    None,
                    None,
                    None,
                );
            }
            "link" => {
                let rel = attr(attrs, "rel").unwrap_or("").to_ascii_lowercase();
                let mut seen = std::collections::HashSet::new();
                for token in rel.split_ascii_whitespace() {
                    if !seen.insert(token) {
                        continue;
                    }
                    if let Some((kind, relation)) = link_kind(token, attrs) {
                        push(
                            kind,
                            relation,
                            Some("href"),
                            (relation == "modulepreload").then_some("module"),
                            None,
                        );
                        if relation == "preload" {
                            push(
                                "image",
                                "imagesrcset",
                                Some("imagesrcset"),
                                None,
                                Some("compound"),
                            );
                        }
                    }
                }
            }
            "img" => {
                push("image", "image", Some("src"), None, None);
                push("image", "srcset", Some("srcset"), None, Some("compound"));
            }
            "input"
                if attr(attrs, "type").is_some_and(|value| value.eq_ignore_ascii_case("image")) =>
            {
                push("image", "image", Some("src"), None, None);
            }
            "audio" | "video" | "source" => {
                push("media", "media", Some("src"), None, None);
                if tag == "video" {
                    push("image", "poster", Some("poster"), None, None);
                }
                if tag == "source" {
                    push("image", "srcset", Some("srcset"), None, Some("compound"));
                }
            }
            "track" => push("data", "track", Some("src"), None, None),
            "iframe" => {
                if attr(attrs, "srcdoc").is_some() {
                    push(
                        "document",
                        "srcdoc",
                        Some("srcdoc"),
                        None,
                        Some("embedded_html"),
                    );
                } else {
                    push("document", "frame", Some("src"), None, None);
                }
            }
            "embed" => push("other", "embed", Some("src"), None, None),
            "object" => push("other", "object", Some("data"), None, None),
            _ => {}
        }
    }
    (result, template, empty_inline)
}

struct Resolved<'a> {
    candidate: Candidate<'a>,
    url: Option<String>,
    resolution: Option<&'static str>,
    content_source_id: Option<String>,
}

pub fn resources(
    input: &Path,
    query: ResourcesQuery,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Vec<u8>> {
    if query
        .kind
        .as_deref()
        .is_some_and(|kind| !RESOURCE_KINDS.contains(&kind))
    {
        return Err(Error::new("INVALID_QUERY", "invalid HTML resource kind"));
    }
    if query
        .location
        .as_deref()
        .is_some_and(|location| !matches!(location, "inline" | "external"))
    {
        return Err(Error::new(
            "INVALID_QUERY",
            "resource location must be inline or external",
        ));
    }
    let document = query
        .base_url
        .as_deref()
        .map(|value| {
            // The base comes from a request argument rather than the bounded source buffer.
            if value.len() > 64 * 1024 {
                return Err(Error::new(
                    "RESOURCE_LIMIT",
                    "document base URL exceeds 64 KiB",
                ));
            }
            let url = Url::parse(value).map_err(|_| {
                Error::new(
                    "INVALID_QUERY",
                    "--base-url requires an absolute HTTP(S) document URL",
                )
            })?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "--base-url requires an HTTP(S) document URL without credentials",
                ));
            }
            Ok(url)
        })
        .transpose()?;
    let path = input
        .to_str()
        .ok_or_else(|| Error::new("INVALID_INPUT", "HTML input path must be UTF-8"))?;
    let (text, hash) = read_source(input, limits, perf)?;
    let (catalog, ids) = parse_catalog(&text, limits, perf)?;
    let (base, template, total, empty_inline, selected) =
        perf.measure("query", || -> Result<_> {
            let (candidates, template, empty_inline) = candidates(&catalog);
            let total = candidates.len();
            let base = document_base(&catalog, &template, document);
            let filter = query
                .filter
                .as_deref()
                .map(|filter| rxd_core::filter::Contains::new(filter, query.case_sensitive));
            let mut selected = Vec::new();
            for candidate in candidates {
                if query
                    .kind
                    .as_deref()
                    .is_some_and(|kind| kind != candidate.kind)
                    || query
                        .location
                        .as_deref()
                        .is_some_and(|location| location != candidate.location())
                    || filter
                        .as_ref()
                        .is_some_and(|filter| !filter.matches(candidate.name))
                {
                    continue;
                }
                let (url, resolution) = if candidate.attribute.is_none() {
                    (None, None)
                } else if let Some(reason) = candidate.unresolved {
                    (None, Some(reason))
                } else {
                    let (url, resolution) = resolve_url(candidate.name, base.url.as_ref());
                    (url, Some(resolution))
                };
                let content_source_id = if query.include_content && candidate.attribute.is_none() {
                    let Detail::Element {
                        content_start,
                        content_end,
                        ..
                    } = catalog.nodes[candidate.node].detail
                    else {
                        unreachable!()
                    };
                    Some(format!(
                        "{:x}",
                        Sha256::digest(catalog.slice(content_start, content_end)?.as_bytes())
                    ))
                } else {
                    None
                };
                selected.push(Resolved {
                    candidate,
                    url,
                    resolution,
                    content_source_id,
                });
            }
            Ok((base, template, total, empty_inline, selected))
        })?;
    let mut output = Output { bytes: Vec::new() };
    perf.measure("serialize", || -> Result<()> {
        output.json(&envelope::header("html", "resources", Some(&hash), Some(text.len()),
            json!({"base_url":query.base_url,"kind":query.kind,"location":query.location,"filter":query.filter,"case":if query.case_sensitive {"sensitive"} else {"insensitive"},"include_content":query.include_content}),
            json!({"path":path,"analysis":"lexical","parser":"html5gum","parser_version":"0.8.4","lexical_profile":"lexical_naive_v1","resource_profile":"resource_candidates_v1","duplicate_attributes":"first","base":{"document_url":base.document.as_ref().map(Url::as_str),"url":base.url.as_ref().map(Url::as_str),"href":base.href,"at":base.node.map(|index| ids[index].as_str()),"status":base.status}}).as_object().unwrap().clone()))?;
        for entry in &selected {
            let candidate = &entry.candidate;
            let node = &catalog.nodes[candidate.node];
            let Detail::Element { tag, attrs, content_start, content_end, end_tag, .. } = &node.detail else { unreachable!() };
            let inline = candidate.attribute.is_none();
            let content = if inline && query.include_content { Some(catalog.slice(*content_start, *content_end)?) } else { None };
            output.json(&envelope::record("resource", RecordMeta {
                at: Some(&ids[candidate.node]),
                parent_at: node.parent.map(|parent| ids[parent].as_str()),
                context_at: Some(&ids[candidate.node]),
                start: Some(node.start), end: Some(node.end), ..Default::default()
            }, json!({
                "kind":candidate.kind,"name":candidate.name,"tag":tag,"location":candidate.location(),
                "relation":candidate.relation,"attribute":candidate.attribute,"url":entry.url,"resolution":entry.resolution,
                "script_type":candidate.script_type,"end_tag":end_tag,
                "content_start":inline.then_some(content_start),"content_end":inline.then_some(content_end),
                "content_bytes":inline.then_some(content_end-content_start),"content":content,"content_source_id":entry.content_source_id,
                "conditions":{"type":attr(attrs,"type"),"language":attr(attrs,"language"),"rel":attr(attrs,"rel"),"as":attr(attrs,"as"),"media":attr(attrs,"media"),
                    "crossorigin":attr(attrs,"crossorigin"),"integrity":attr(attrs,"integrity"),"referrerpolicy":attr(attrs,"referrerpolicy"),"fetchpriority":attr(attrs,"fetchpriority"),
                    "async":attr(attrs,"async").is_some(),"defer":attr(attrs,"defer").is_some(),"nomodule":attr(attrs,"nomodule").is_some(),"disabled":attr(attrs,"disabled").is_some(),"in_template":template[candidate.node]}
            }).as_object().unwrap().clone()))?;
        }
        output.json(&envelope::summary(selected.len(), true, json!({
            "resources":total,"empty_inline":empty_inline,
            "inline":selected.iter().filter(|entry| entry.candidate.attribute.is_none()).count(),
            "external":selected.iter().filter(|entry| entry.candidate.attribute.is_some()).count(),
            "unresolved":selected.iter().filter(|entry| entry.candidate.attribute.is_some() && entry.url.is_none()).count(),
            "tokenizer_errors":catalog.tokenizer_errors,"unmatched_end_tags":catalog.unmatched_end_tags,"missing_end_tags":catalog.missing_end_tags
        }).as_object().unwrap().clone()))
    })?;
    perf.counters.records = Some(selected.len());
    perf.counters.output_bytes = Some(output.bytes.len());
    Ok(output.bytes)
}
