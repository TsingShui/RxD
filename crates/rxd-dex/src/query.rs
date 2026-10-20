//! The search query model shared by the CLI and the scanners.
//!
//! `cli` builds these values from command-line arguments; `apk` and `dex` consume
//! them. Keeping the model in its own module means the analysis layers never have
//! to depend on the argument-parsing layer.

use anyhow::{Result, bail};

/// What an `xref` invocation looks for.
#[derive(Clone, Debug)]
pub enum Query {
    String(Pattern),
    Type(Pattern),
    Method(MemberQuery),
    Field(MemberQuery),
}

/// One selector face.
///
/// The two faces answer different questions, so they are spelled differently on the command
/// line. `-f/--filter` is a content search: a case-sensitive substring, where any entry whose
/// table holds a match may answer and zero rows is a true answer. `--at` is a coordinate that
/// a row already printed: it names one identity, matched exactly, and an identity that no
/// entry carries is an error rather than an empty result.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub text: String,
    pub exact: bool,
    /// `false` (the default) folds case: a filter narrows a list, and a missed capital
    /// costs a round trip. `--case sensitive` is what turns folding off.
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

/// A member query: at least one of a member name, a declaring class, or the descriptor an
/// exact coordinate carries.
#[derive(Clone, Debug)]
pub struct MemberQuery {
    pub name: Option<Pattern>,
    /// A case-sensitive substring of the descriptor, in `pkg/Name` form, where dots are
    /// separators.
    pub class: Option<Pattern>,
    /// The prototype of a method coordinate (`(I)V`) or the type of a field coordinate
    /// (`Ljava/lang/String;`). Only an exact anchor sets it: a name plus its prototype is
    /// what names one method exactly.
    pub descriptor: Option<String>,
}

/// Normalizes a class name to the `Lpkg/Name;` descriptor form that `findrefs`
/// and `getclass` compare against the DEX tables.
///
/// Accepts dotted Java names, descriptors with or without their `L`/`;`
/// delimiters, and rejects the empty name.
pub fn format_class_name(name: &str) -> Result<String> {
    if name.is_empty() {
        bail!("Class name cannot be empty");
    }
    // A name that already looks like a descriptor is used as written, but only when
    // its slashes are there too: `Lcom.foo.Main;` still needs its dots normalised.
    // This mirrors the reference implementation exactly.
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

/// Normalizes a fuzzy class pattern by treating dots as package separators.
///
/// The reference implementation only replaces dots when the pattern has no slashes,
/// but that rule exists for its regex matcher, where a dot is a wildcard and
/// `com.example/Main` therefore matches `com/example/Main` by accident. rasc matches
/// literally, so leaving such a pattern alone would match nothing; replacing the dots
/// gives the same rows the reference reports for those inputs.
pub fn fuzzy_class_pattern(pattern: &str) -> String {
    pattern.replace('.', "/")
}

impl Query {
    /// The kind this query reports, as the CLI spells it.
    pub fn kind(&self) -> &'static str {
        match self {
            Query::String(_) => "string",
            Query::Type(_) => "type",
            Query::Method(_) => "method",
            Query::Field(_) => "field",
        }
    }

    /// Whether this query came from an `--at` coordinate rather than a content search.
    ///
    /// Only an anchor promises that its identity exists: a content search that matches
    /// nothing answers with an empty result set, while an anchor that resolves nowhere is a
    /// `NOT_FOUND` error naming what was looked for.
    pub fn is_anchor(&self) -> bool {
        match self {
            Query::String(pattern) | Query::Type(pattern) => pattern.exact,
            Query::Method(query) | Query::Field(query) => query.descriptor.is_some(),
        }
    }

    /// The error text for an anchor that no entry resolved, naming what was looked for.
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

    /// Whether a class constraint makes the member table small enough that a per-entry
    /// prefix pre-check pays for its read.
    ///
    /// This is a cost heuristic, not a correctness rule: `prefix_has_targets` resolves the
    /// same query against the entry's tables, so its answer is exactly "would this entry
    /// produce a row". A name-only query matches most entries of a real APK, where the
    /// prefix read is pure overhead, so it is skipped.
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

    /// Every case here was read off the reference implementation's
    /// `_format_class_name` / `_normalize_class_query`.
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
