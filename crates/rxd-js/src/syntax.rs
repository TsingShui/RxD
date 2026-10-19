//! Syntax selection is independent of script/module mode; source is never transformed.

use std::path::Path;

use oxc_span::SourceType;
use serde::Serialize;

use crate::SourceMode;

#[derive(Clone, Copy)]
pub enum Syntax {
    Auto,
    Js,
    Jsx,
    Ts,
    Tsx,
}

#[derive(Clone, Copy, Serialize)]
pub struct Info {
    pub syntax: Option<&'static str>,
    pub declaration_file: Option<bool>,
}

impl Info {
    pub fn new(source_type: SourceType) -> Self {
        let syntax = match (source_type.is_typescript(), source_type.is_jsx()) {
            (true, true) => Some("tsx"),
            (true, false) => Some("ts"),
            (false, true) => Some("jsx"),
            _ => None,
        };
        Self {
            syntax,
            declaration_file: syntax.map(|_| source_type.is_typescript_definition()),
        }
    }
}

pub fn select(path: &Path, mode: SourceMode, syntax: Syntax) -> SourceType {
    let extension = path.extension().and_then(|s| s.to_str());
    let base = match mode {
        SourceMode::Script => SourceType::cjs(),
        SourceMode::Module => SourceType::mjs(),
        SourceMode::Auto => match extension {
            Some("mjs" | "mts") => SourceType::mjs(),
            Some("cjs" | "cts") => SourceType::cjs(),
            _ => SourceType::unambiguous(),
        },
    };
    let syntax = match syntax {
        Syntax::Auto => match extension {
            Some("jsx") => Syntax::Jsx,
            Some("ts" | "mts" | "cts") => Syntax::Ts,
            Some("tsx") => Syntax::Tsx,
            _ => Syntax::Js,
        },
        syntax => syntax,
    };
    let typed = matches!(syntax, Syntax::Ts | Syntax::Tsx);
    base.with_typescript(typed)
        .with_jsx(matches!(syntax, Syntax::Jsx | Syntax::Tsx))
        .with_typescript_definition(
            typed && SourceType::from_path(path).is_ok_and(|s| s.is_typescript_definition()),
        )
}
