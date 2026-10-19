use super::Request;
#[cfg(feature = "kuna")]
use super::Target;
use crate::{Diagnostic, ErrorKind, Language, Limits, Result, Stage};

#[cfg(feature = "kuna")]
pub(super) use rxd_kuna_cli::hook::Signature;
#[cfg(not(feature = "kuna"))]
pub(super) struct Signature;

pub(super) fn inspect(
    request: &Request,
    known: Option<u64>,
    limits: Limits,
) -> Result<(u64, Option<Signature>)> {
    let typed = request.options.language == Language::TypeScript;
    if let Some(address) = known
        && (!typed || request.options.specs_dir.is_none())
    {
        return Ok((address, None));
    }
    #[cfg(feature = "kuna")]
    if let Some(specs) = &request.options.specs_dir {
        use rxd_kuna_cli::hook::{self, EntryTarget, Failure};
        use std::time::Duration;
        if !specs.is_dir() {
            return Err(Diagnostic::new(
                ErrorKind::EngineUnavailable,
                Stage::Analysis,
                "prepared Kuna specs directory is unavailable",
            ));
        }
        let directory = tempfile::tempdir()
            .map_err(|e| Diagnostic::new(ErrorKind::Io, Stage::Input, e.to_string()))?;
        let snapshot = directory.path().join("image");
        std::fs::write(&snapshot, request.input.bytes())
            .map_err(|e| Diagnostic::new(ErrorKind::Io, Stage::Input, e.to_string()))?;
        let target = match (&request.target, known) {
            (_, Some(address)) => EntryTarget::Address(address),
            (Target::Name(name), _) => EntryTarget::Name(name),
            (Target::StaticAddress(address), _) => EntryTarget::Address(*address),
        };
        let selection = hook::inspect(
            request.input.bytes(),
            &snapshot,
            specs,
            target,
            typed,
            limits.analysis_steps,
            limits.expanded_bytes,
            Duration::from_millis(limits.analysis_timeout_ms),
        )
        .map_err(|failure| {
            let (kind, message) = match failure {
                Failure::Invalid(message) => (ErrorKind::InvalidInput, message),
                Failure::Missing => (
                    ErrorKind::NotFound,
                    "no recognized function entry at target; interior instructions are unsupported"
                        .into(),
                ),
                Failure::Ambiguous => (
                    ErrorKind::Ambiguous,
                    "multiple recognized function entries match target".into(),
                ),
                Failure::Budget => (
                    ErrorKind::ResourceLimit,
                    "Kuna expansion/work budget exceeded".into(),
                ),
                Failure::Timeout => (
                    ErrorKind::AnalysisTimeout,
                    "Kuna analysis elapsed-time budget exceeded".into(),
                ),
                Failure::Analysis(message) => (
                    ErrorKind::Unsupported,
                    format!("Kuna selected analysis failed: {message}"),
                ),
            };
            Diagnostic::new(kind, Stage::Analysis, message)
        })?;
        return Ok((selection.address, selection.signature));
    }
    let _ = limits;
    Err(Diagnostic::new(
        ErrorKind::EngineUnavailable,
        Stage::Selection,
        "target requires linked Kuna and an explicit prepared specs directory; no function entry or signature is invented",
    ))
}
