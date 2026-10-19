use super::{Emit, Request};
use crate::{ArtifactFile, Diagnostic, ErrorKind, ExtensionArtifacts, Limits, Result, Stage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    manifest_version: u8,
    name: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    minimum_chrome_version: String,
    content_scripts: Vec<ContentScript>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContentScript {
    matches: Vec<String>,
    js: Vec<String>,
    world: String,
    run_at: String,
    all_frames: bool,
}

fn invalid(message: &str) -> Diagnostic {
    Diagnostic::new(ErrorKind::InvalidArguments, Stage::Render, message)
}

// Supported Chrome match grammar: explicit <all_urls>, http/https/file/ftp/*,
// ASCII DNS/IPv4 hosts, * or *.host, and a slash-prefixed path. No ports/credentials.
pub(super) fn valid_match(pattern: &str) -> bool {
    if pattern == "<all_urls>" {
        return true;
    }
    if pattern
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return false;
    }
    let Some((scheme, tail)) = pattern.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https" | "file" | "ftp" | "*") {
        return false;
    }
    let Some((host, _path)) = tail.split_once('/') else {
        return false;
    };
    if scheme == "file" {
        return host.is_empty();
    }
    if host == "*" {
        return true;
    }
    let host = host.strip_prefix("*.").unwrap_or(host);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

pub(super) fn check_options(request: &Request, limits: Limits) -> Result<()> {
    match request.emit {
        Emit::Snippet if !request.matches.is_empty() => {
            return Err(invalid("match patterns require extension output"));
        }
        Emit::Extension if request.matches.is_empty() => {
            return Err(invalid("extension output requires explicit match patterns"));
        }
        _ => {}
    }
    let mut bytes = 0usize;
    for pattern in &request.matches {
        limits.check_target(pattern)?;
        bytes = bytes.checked_add(pattern.len().max(1)).ok_or_else(|| {
            Diagnostic::new(
                ErrorKind::ResourceLimit,
                Stage::Render,
                "match byte count overflow",
            )
        })?;
        if bytes > limits.expanded_bytes.min(limits.artifact_bytes) {
            return Err(Diagnostic::new(
                ErrorKind::ResourceLimit,
                Stage::Render,
                "match expansion budget exceeded",
            ));
        }
        if !valid_match(pattern) {
            return Err(invalid("invalid or unsupported Chrome page match pattern"));
        }
    }
    Ok(())
}

pub(super) fn assemble(
    request: &Request,
    source: String,
    limits: Limits,
) -> Result<ExtensionArtifacts> {
    let manifest = Manifest {
        manifest_version: 3,
        name: "rxd Web hook".into(),
        version: "1.0.0".into(),
        description: Some("Offline generated property-call observation hook".into()),
        minimum_chrome_version: "111".into(),
        content_scripts: vec![ContentScript {
            matches: request.matches.clone(),
            js: vec!["hook.js".into()],
            world: "MAIN".into(),
            run_at: "document_start".into(),
            all_frames: false,
        }],
    };
    let maximum = limits
        .artifact_bytes
        .min(limits.expanded_bytes)
        .saturating_sub(source.len());
    let mut buffer = ManifestBuffer {
        bytes: Vec::new(),
        maximum,
    };
    serde_json::to_writer_pretty(&mut buffer, &manifest).map_err(|_| {
        Diagnostic::new(
            ErrorKind::ResourceLimit,
            Stage::Render,
            "manifest/artifact expansion byte budget exceeded",
        )
    })?;
    let contents = buffer.bytes;
    let artifacts = ExtensionArtifacts::new(
        vec![
            ArtifactFile {
                name: "manifest.json".into(),
                contents,
            },
            ArtifactFile {
                name: "hook.js".into(),
                contents: source.into_bytes(),
            },
        ],
        limits,
    )?;
    validate(&artifacts, limits)?;
    Ok(artifacts)
}

pub(super) fn validate(artifacts: &ExtensionArtifacts, limits: Limits) -> Result<()> {
    artifacts.validate(limits)?;
    let bytes = &artifacts
        .files
        .iter()
        .find(|file| file.name == "manifest.json")
        .expect("validated manifest presence")
        .contents;
    // serde's recursion ceiling remains enabled for hostile caller-edited manifests.
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|_| invalid("invalid or unrequested extension manifest fields"))?;
    if manifest.manifest_version != 3
        || manifest.name.is_empty()
        || manifest.name.chars().count() > 75
        || !version(&manifest.version, true)
        || !version(&manifest.minimum_chrome_version, false)
        || manifest
            .minimum_chrome_version
            .split('.')
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .is_none_or(|major| major < 111)
        || manifest.content_scripts.len() != 1
    {
        return Err(invalid(
            "invalid MV3 metadata; static MAIN content scripts require Chrome >=111",
        ));
    }
    let script = &manifest.content_scripts[0];
    if script.world != "MAIN"
        || script.run_at != "document_start"
        || script.all_frames
        || script.matches.is_empty()
        || script.matches.iter().any(|pattern| !valid_match(pattern))
        || script.js.is_empty()
    {
        return Err(invalid(
            "expected explicit matched MAIN/document_start/top-frame local JS content script",
        ));
    }
    for pattern in &script.matches {
        limits.check_target(pattern)?;
    }
    let mut referenced = BTreeSet::new();
    let mut portable_names = BTreeSet::new();
    for name in &script.js {
        let stem = name
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches([' ', '.'])
            .to_uppercase();
        let device = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
        if !name.ends_with(".js")
            || device
            || name.ends_with([' ', '.'])
            || !portable_names.insert(name.to_lowercase())
            || !referenced.insert(name.as_str())
        {
            return Err(invalid("expected distinct local JS payload files"));
        }
        let file = artifacts
            .files
            .iter()
            .find(|file| &file.name == name)
            .ok_or_else(|| invalid("missing local payload"))?;
        if file.contents.is_empty() || std::str::from_utf8(&file.contents).is_err() {
            return Err(invalid(
                "JS payload must be nonempty UTF-8; publication does not parse source",
            ));
        }
    }
    if artifacts.files.len() != referenced.len() + 1 {
        return Err(invalid("unreferenced extension files are unsupported"));
    }
    Ok(())
}

// Enforce the complete output ceiling while serializing, not after an unbounded
// temporary JSON allocation. Serialization of these concrete fields is infallible
// except for this writer's byte ceiling.
struct ManifestBuffer {
    bytes: Vec<u8>,
    maximum: usize,
}
impl std::io::Write for ManifestBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("manifest byte budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn version(value: &str, extension: bool) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    !parts.is_empty()
        && parts.len() <= 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|c| c.is_ascii_digit())
                && (!extension || part.len() == 1 || !part.starts_with('0'))
                && part.parse::<u32>().is_ok_and(|v| !extension || v <= 65535)
        })
        && (!extension || parts.iter().any(|part| *part != "0"))
}
