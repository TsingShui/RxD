use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidArguments,
    InvalidInput,
    NotFound,
    Ambiguous,
    Unsupported,
    ProviderUnavailable,
    EngineUnavailable,
    ResourceLimit,
    AnalysisTimeout,
    Io,
}

impl ErrorKind {
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::InvalidArguments => 2,
            Self::NotFound => 3,
            Self::Ambiguous => 4,
            _ => 1,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidArguments => "INVALID_ARGUMENTS",
            Self::InvalidInput => "INVALID_INPUT",
            Self::NotFound => "TARGET_NOT_FOUND",
            Self::Ambiguous => "TARGET_AMBIGUOUS",
            Self::Unsupported => "UNSUPPORTED",
            Self::ProviderUnavailable => "PROVIDER_UNAVAILABLE",
            Self::EngineUnavailable => "ENGINE_UNAVAILABLE",
            Self::ResourceLimit => "RESOURCE_LIMIT",
            Self::AnalysisTimeout => "ANALYSIS_TIMEOUT",
            Self::Io => "IO_ERROR",
        }
    }
}

/// Generation phases and generated setup stages share stable, target-qualified labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Input,
    Selection,
    Analysis,
    Render,
    Publication,
    DexSchedule,
    DexJavaUse,
    DexOverload,
    DexImplementation,
    NativeModule,
    NativeTarget,
    NativeAttach,
    WebTarget,
    WebReplaceability,
    WebInstall,
    WebWait,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Selection => "selection",
            Self::Analysis => "analysis",
            Self::Render => "render",
            Self::Publication => "publication",
            Self::DexSchedule => "dex.schedule",
            Self::DexJavaUse => "dex.java-use",
            Self::DexOverload => "dex.overload",
            Self::DexImplementation => "dex.implementation",
            Self::NativeModule => "native.module",
            Self::NativeTarget => "native.target",
            Self::NativeAttach => "native.attach",
            Self::WebTarget => "web.target",
            Self::WebReplaceability => "web.replaceability",
            Self::WebInstall => "web.install",
            Self::WebWait => "web.wait",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: ErrorKind,
    pub stage: Stage,
    pub target: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(kind: ErrorKind, stage: Stage, message: impl Into<String>) -> Self {
        Self {
            kind,
            stage,
            target: None,
            message: message.into(),
        }
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub const fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Serialize untrusted text to keep a diagnostic on one physical line.
        write!(f, "{} stage={}", self.kind.code(), self.stage.as_str())?;
        if let Some(target) = &self.target {
            write!(f, " target={}", crate::source::js_string(target))?;
        }
        write!(f, " message={}", crate::source::js_string(&self.message))
    }
}
impl std::error::Error for Diagnostic {}
