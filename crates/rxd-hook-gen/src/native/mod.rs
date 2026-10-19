//! Native provider boundary; mapping and selected-function analysis belong to the native lane.
use crate::{Artifact, CapturedInput, Language, Limits, Result};
use crate::{Diagnostic, ErrorKind, Stage};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Target {
    Name(String),
    /// Static virtual address, never a file offset or a runtime absolute pointer.
    StaticAddress(u64),
}

#[derive(Clone, Debug, Default)]
pub enum RuntimeModule {
    #[default]
    Auto,
    Main,
    Named(String),
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub language: Language,
    pub module: RuntimeModule,
    /// Explicit caller-prepared specs. No package fetching or global environment mutation.
    /// None does not disable cheap known-entry generation; unavailable required analysis fails explicitly.
    pub specs_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub input: CapturedInput,
    pub target: Target,
    pub options: Options,
}

#[cfg(feature = "native")]
mod engine;
#[cfg(feature = "native")]
mod image;
#[cfg(feature = "native")]
mod render;
#[cfg(feature = "native")]
mod types;

pub fn generate(request: &Request, limits: Limits) -> Result<Artifact> {
    limits.validate()?;
    limits.check_input(request.input.bytes().len())?;
    if let Target::Name(name) = &request.target {
        limits.check_target(name)?;
        if name.is_empty() || name.contains('\0') {
            return Err(Diagnostic::new(
                ErrorKind::InvalidArguments,
                Stage::Selection,
                "function name is empty or contains NUL",
            ));
        }
    }
    if let RuntimeModule::Named(name) = &request.options.module {
        limits.check_target(name)?;
    }
    #[cfg(feature = "native")]
    {
        let image = image::resolve(request, limits)?;
        render::render(request, &image, limits)
    }
    #[cfg(not(feature = "native"))]
    Err(Diagnostic::new(
        ErrorKind::ProviderUnavailable,
        Stage::Selection,
        "native provider is disabled",
    ))
}
