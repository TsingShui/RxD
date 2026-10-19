//! Offline hook generation from captured input or explicit page-property options.
//! Providers never execute inputs or generated source. This module does not wire CLI dispatch.
mod artifact;
mod diagnostic;
mod input;
mod limits;
pub mod source;

pub mod dex;
pub mod native;
pub mod web;

pub use artifact::{
    Artifact, ArtifactFile, ExtensionArtifacts, PublicationReceipt, SourceArtifact,
};
pub use diagnostic::{Diagnostic, ErrorKind, Stage};
pub use input::CapturedInput;
pub use limits::{AnalysisBudget, Limits};
pub use web::publish_extension;

pub type Result<T> = std::result::Result<T, Diagnostic>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    JavaScript,
    TypeScript,
}

impl std::str::FromStr for Language {
    type Err = Diagnostic;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "js" => Ok(Self::JavaScript),
            "ts" => Ok(Self::TypeScript),
            _ => Err(Diagnostic::new(
                ErrorKind::InvalidArguments,
                Stage::Input,
                "language must be js or ts",
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Request {
    Dex(dex::Request),
    Native(native::Request),
    Web(web::Request),
}

/// Return one complete artifact or one diagnostic; errors never contain partial source.
pub fn generate(request: &Request, limits: Limits) -> Result<Artifact> {
    limits.validate()?;
    let artifact = match request {
        Request::Dex(request) => {
            limits.check_input(request.input.bytes().len())?;
            match &request.selector {
                dex::Selector::Signature(signature) => limits.check_target(signature)?,
                dex::Selector::MethodIndex { class, .. } => limits.check_target(class)?,
            }
            if let Some(entry) = &request.options.entry {
                limits.check_target(entry)?;
            }
            dex::generate(request, limits)?
        }
        Request::Native(request) => {
            limits.check_input(request.input.bytes().len())?;
            if let native::Target::Name(name) = &request.target {
                limits.check_target(name)?;
            }
            if let native::RuntimeModule::Named(name) = &request.options.module {
                limits.check_target(name)?;
            }
            native::generate(request, limits)?
        }
        Request::Web(request) => {
            limits.check_target(&request.target)?;
            for pattern in &request.matches {
                limits.check_target(pattern)?;
            }
            web::generate(request, limits)?
        }
    };
    // Providers may assemble editable public artifact structs directly; enforce the final
    // complete-output budget here as well as during bounded source construction.
    match &artifact {
        Artifact::Source(source) => {
            limits.check_source(source.source.len())?;
            limits::check(
                source.source.len(),
                limits.artifact_bytes,
                Stage::Render,
                "artifact byte budget exceeded",
            )?;
        }
        Artifact::Extension(extension) => extension.validate(limits)?,
    }
    Ok(artifact)
}
