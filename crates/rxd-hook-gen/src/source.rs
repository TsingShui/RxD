//! Source assembly helpers. Only trusted template text goes to `push`; all data goes
//! through `js_string`. Providers own an enclosing local scope for the fixed setup names.
use crate::{Diagnostic, ErrorKind, Language, Limits, Result, Stage};

pub fn js_string(value: &str) -> String {
    escape_serialized_json(
        serde_json::to_string(value).expect("string serialization is infallible"),
    )
}

// Preserve the serialized value's type while making it safe to embed in source.
pub(crate) fn escape_serialized_json(json: String) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub struct SourceBuilder {
    source: String,
    limits: Limits,
}

impl SourceBuilder {
    pub fn new(limits: Limits) -> Result<Self> {
        limits.validate()?;
        Ok(Self {
            source: String::new(),
            limits,
        })
    }

    pub fn push(&mut self, trusted_source: &str) -> Result<()> {
        let size = self
            .source
            .len()
            .checked_add(trusted_source.len())
            .ok_or_else(|| {
                Diagnostic::new(
                    ErrorKind::ResourceLimit,
                    Stage::Render,
                    "source byte count overflow",
                )
            })?;
        self.limits.check_source(size)?;
        self.source.push_str(trusted_source);
        Ok(())
    }

    pub fn finish(self) -> String {
        self.source
    }
}

#[derive(Clone, Copy, Debug)]
pub enum StageEvent {
    Begin,
    Success,
    Waiting,
    Timeout,
}
impl StageEvent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Begin => "begin",
            Self::Success => "success",
            Self::Waiting => "waiting",
            Self::Timeout => "timeout",
        }
    }
}

/// Diagnostic sink failures must not change target return/throw behavior.
/// The provider must keep target invocation outside its setup try/catch.
pub fn setup_prelude(target: &str, language: Language) -> String {
    let args = match language {
        Language::JavaScript => "...values",
        Language::TypeScript => "...values: unknown[]",
    };
    format!(
        "const __rxdTarget = {};\nlet __rxdStage = \"initialization\";\nconst __rxdConsoleLog = (() => {{\n  try {{ return console.log.bind(console); }} catch {{ return () => {{}}; }}\n}})();\nconst __rxdLog = ({args}) => {{\n  try {{ __rxdConsoleLog(\"[rxd:hook-gen]\", __rxdTarget, __rxdStage, ...values); }} catch {{}}\n}};\n",
        js_string(target)
    )
}

pub fn setup_stage(stage: Stage, event: StageEvent) -> String {
    format!(
        "__rxdStage = {};\n__rxdLog({});\n",
        js_string(stage.as_str()),
        js_string(event.as_str())
    )
}

/// Includes the catch clause, logs a narrowed unknown error and rethrows the exact value.
pub fn setup_catch() -> &'static str {
    "catch (__rxdError) {\n  __rxdLog(\"failure\", __rxdError instanceof Error ? (__rxdError.stack ?? __rxdError.message) : __rxdError);\n  throw __rxdError;\n}\n"
}
