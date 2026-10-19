//! Browser property provider boundary; parsing, rendering and publication belong to the Web lane.
use crate::source::{SourceBuilder, js_string, setup_prelude};
use crate::{AnalysisBudget, Language, SourceArtifact};
use crate::{
    Artifact, Diagnostic, ErrorKind, ExtensionArtifacts, Limits, PublicationReceipt, Result, Stage,
};
use std::path::Path;
mod extension;
mod path;
mod publication;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Emit {
    #[default]
    Snippet,
    Extension,
}

impl std::str::FromStr for Emit {
    type Err = Diagnostic;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "snippet" => Ok(Self::Snippet),
            "extension" => Ok(Self::Extension),
            _ => Err(Diagnostic::new(
                ErrorKind::InvalidArguments,
                Stage::Input,
                "Web emit must be snippet or extension",
            )),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Request {
    /// Page-global path: ASCII identifiers or JSON double-quoted bracket keys.
    /// window/self aliases normalize to globalThis; implicit roots are page-global.
    /// __proto__ and constructor traversal is rejected; explicit prototype is allowed.
    pub target: String,
    pub emit: Emit,
    pub matches: Vec<String>,
    /// Bounded polling in emitted source; generation itself never waits for a page.
    pub wait_ms: Option<u32>,
}

pub fn generate(request: &Request, limits: Limits) -> Result<Artifact> {
    limits.validate()?;
    limits.check_target(&request.target)?;
    if !cfg!(feature = "web") {
        return Err(Diagnostic::new(
            ErrorKind::ProviderUnavailable,
            Stage::Selection,
            "Web provider requires the web feature",
        )
        .with_target(&request.target));
    }
    crate::limits::check(
        request.target.len(),
        limits.expanded_bytes,
        Stage::Render,
        "Web expansion budget exceeded",
    )?;
    let mut budget = AnalysisBudget::new(limits)?;
    budget.consume(request.target.len() as u64)?;
    for pattern in &request.matches {
        budget.consume(pattern.len().max(1) as u64)?;
    }
    extension::check_options(request, limits)?;
    let path = path::parse(&request.target)?;
    if request
        .wait_ms
        .is_some_and(|wait| wait == 0 || wait > i32::MAX as u32)
    {
        return Err(Diagnostic::new(
            ErrorKind::InvalidArguments,
            Stage::WebWait,
            "wait must be 1..2147483647 milliseconds",
        ));
    }
    let mut source = SourceBuilder::new(limits)?;
    source.push("(() => {\n\"use strict\";\n")?;
    source.push(&setup_prelude(&path.normalized, Language::JavaScript))?;
    let keys = path
        .keys
        .iter()
        .map(|key| js_string(key))
        .collect::<Vec<_>>()
        .join(", ");
    source.push(&format!("const keys = [{keys}];\n"))?;
    source.push(include_str!("hook.js"))?;
    if let Some(wait) = request.wait_ms {
        source.push(&format!("const waitMs = {wait};\n"))?;
        source.push(include_str!("wait.js"))?;
    } else {
        source.push("try {\n  if (!install()) throw new Error(\"target property not found; inaccessible lexical bindings are unsupported\");\n} catch (error) {\n  __rxdLog(\"failure\", error);\n  throw error;\n}\n")?;
    }
    source.push("})();\n")?;
    let source = source.finish();
    crate::limits::check(
        source.len(),
        limits.expanded_bytes,
        Stage::Render,
        "Web expansion budget exceeded",
    )?;
    let artifact = SourceArtifact::new(Language::JavaScript, source, limits)?;
    let artifact = match request.emit {
        Emit::Snippet => Artifact::Source(artifact),
        Emit::Extension => {
            Artifact::Extension(extension::assemble(request, artifact.source, limits)?)
        }
    };
    budget.consume(1)?;
    Ok(artifact)
}

/// Publish structurally validated local MV3 artifacts atomically. This checks the
/// manifest subset, exact local references, UTF-8 JS files and budgets, not JS syntax,
/// semantics or runtime hookability. Generation E2E validates source separately offline.
///
/// Private sibling staging precedes atomic no-clobber directory rename: Linux uses
/// renameat2/RENAME_NOREPLACE (kernel/filesystem support required), macOS uses
/// renameatx_np/RENAME_EXCL, Windows uses MoveFileExW without replacement/copy flags.
/// Other platforms fail explicitly; errors never fall back to a clobbering rename.
/// Nonempty directories, files and final symlinks are never replaced. Existing empty
/// directories may be removed; a racing destination causes publication to fail.
///
/// The parent must already exist. Parent symlinks (including macOS /tmp) are resolved
/// once; callers must control the parent/ancestors against concurrent renames. This
/// is atomic visibility, not a filesystem sandbox or power-loss durability guarantee.
pub fn publish_extension(
    artifacts: &ExtensionArtifacts,
    destination: &Path,
    limits: Limits,
) -> Result<PublicationReceipt> {
    if !cfg!(feature = "web") {
        return Err(Diagnostic::new(
            ErrorKind::ProviderUnavailable,
            Stage::Publication,
            "Web publication requires the web feature",
        ));
    }
    publication::publish(artifacts, destination, limits)
}
