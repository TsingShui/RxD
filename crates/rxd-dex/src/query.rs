//! Shared selectors for CLI dispatch and DEX scanners.

use anyhow::{Result, bail};

#[derive(Clone, Debug)]
pub enum Query {
    String(Pattern),
    Type(Pattern),
    Method(MemberQuery),
    Field(MemberQuery),
}

/// Substring search or exact anchor; unresolved anchors are errors, not zero hits.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub text: String,
    pub exact: bool,
    /// False folds substring case; coordinates remain exact.
    pub case_sensitive: bool,
}

impl Pattern {
    /// A content search: a substring, folded unless `--case sensitive`.
    pub fn search(text: impl Into<String>, case_sensitive: bool) -> Self {
        Self {
            text: text.into(),
            exact: false,
            case_sensitive,
        }
    }

    /// A coordinate a row printed: one identity, matched exactly.
    pub fn coordinate(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            exact: true,
            case_sensitive: true,
        }
    }
}

/// Member selector by name, owner or exact descriptor.
#[derive(Clone, Debug)]
pub struct MemberQuery {
    pub name: Option<Pattern>,
    /// Owner selector in pkg/Name form; dots normalize to separators.
    pub class: Option<Pattern>,
    /// Exact method prototype or field type; distinguishes overloaded coordinates.
    pub descriptor: Option<String>,
}

/// Normalize dotted/descriptive class names to Lpkg/Name;; reject empty names.
pub fn format_class_name(name: &str) -> Result<String> {
    if name.is_empty() {
        bail!("Class name cannot be empty");
    }
    // Only slash-qualified descriptors bypass dot normalization.
    if name.starts_with('L') && name.ends_with(';') && name.contains('/') {
        return Ok(name.to_owned());
    }
    let mut name = name.replace('.', "/");
    if !name.starts_with('L') {
        name.insert(0, 'L');
    }
    if !name.ends_with(';') {
        name.push(';');
    }
    Ok(name)
}

/// Treat dots as literal package separators, including mixed dotted/slash patterns.
pub fn fuzzy_class_pattern(pattern: &str) -> String {
    pattern.replace('.', "/")
}

impl Query {
    pub fn kind(&self) -> &'static str {
        match self {
            Query::String(_) => "string",
            Query::Type(_) => "type",
            Query::Method(_) => "method",
            Query::Field(_) => "field",
        }
    }

    /// Anchors require identity existence; substring searches permit zero hits.
    pub fn is_anchor(&self) -> bool {
        match self {
            Query::String(pattern) | Query::Type(pattern) => pattern.exact,
            Query::Method(query) | Query::Field(query) => query.descriptor.is_some(),
        }
    }

    pub fn anchor_not_found(&self) -> String {
        match self {
            Query::String(pattern) => {
                format!("no string literal is exactly {:?}", pattern.text)
            }
            Query::Type(pattern) => {
                format!("no type is named {}", pattern.text)
            }
            Query::Method(query) | Query::Field(query) => {
                let name = match self {
                    Query::Method(_) => "method",
                    _ => "field",
                };
                let member = match (&query.name, &query.descriptor) {
                    (Some(name), Some(descriptor)) => format!("{}{}", name.text, descriptor),
                    (Some(name), None) => name.text.clone(),
                    (None, _) => String::new(),
                };
                match &query.class {
                    Some(class) => format!("no {name} {} matches {}", member, class.text),
                    None => format!("no {name} named {member}"),
                }
            }
        }
    }

    /// Class constraints make prefix prechecks worthwhile; this is only a cost heuristic.
    pub fn bounded_by_class(&self) -> bool {
        match self {
            Query::Method(query) | Query::Field(query) => query.class.is_some(),
            Query::String(_) | Query::Type(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_class_names_like_the_reference() {
        for (input, expected) in [
            ("com.foo.Main", "Lcom/foo/Main;"),
            ("Lcom/foo/Main;", "Lcom/foo/Main;"),
            ("Lcom.foo.Main;", "Lcom/foo/Main;"),
            ("LMain;", "LMain;"),
            ("com/foo/Main", "Lcom/foo/Main;"),
            ("Main", "LMain;"),
            ("Lcom/foo/Main", "Lcom/foo/Main;"),
            ("com.foo.Main$Inner", "Lcom/foo/Main$Inner;"),
            ("Lcom/foo/Main$Inner;", "Lcom/foo/Main$Inner;"),
            ("I", "LI;"),
            ("[Lcom/foo/Main;", "L[Lcom/foo/Main;"),
            ("java.lang.String", "Ljava/lang/String;"),
            ("Lfoo.bar", "Lfoo/bar;"),
            ("foo.bar;", "Lfoo/bar;"),
        ] {
            assert_eq!(format_class_name(input).unwrap(), expected, "{input:?}");
        }
        assert!(format_class_name("").is_err());
    }

    #[test]
    fn normalizes_fuzzy_class_patterns_like_the_reference() {
        for (input, expected) in [
            ("com.foo", "com/foo"),
            ("com/foo", "com/foo"),
            ("com.foo/bar", "com/foo/bar"),
            ("com/foo.bar", "com/foo/bar"),
            ("Main", "Main"),
            ("", ""),
            ("Lcom/foo/Main;", "Lcom/foo/Main;"),
        ] {
            assert_eq!(fuzzy_class_pattern(input), expected, "{input:?}");
        }
    }
}
