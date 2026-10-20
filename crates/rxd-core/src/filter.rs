//! Case-folded matching for every filter, in one place.
//!
//! Every backend's `-f/--filter` answers the same question — "does this label contain the
//! pattern?" — so the folding rule lives here rather than once per crate. The default is
//! case-**insensitive**: a filter narrows a list the caller is already looking at, and a
//! missed capital costs a round trip. `--case sensitive` switches to exact matching, which
//! is what a query over identities needs.

/// A needle matched against labels.
#[derive(Clone, Debug)]
pub struct Contains {
    needle: String,
    /// The lowered needle; `None` when matching is case-sensitive.
    folded: Option<String>,
}

impl Contains {
    pub fn new(needle: &str, case_sensitive: bool) -> Self {
        Self {
            needle: needle.to_owned(),
            folded: (!case_sensitive).then(|| needle.to_lowercase()),
        }
    }

    /// The needle as the caller spelled it, for header echoes.
    pub fn text(&self) -> &str {
        &self.needle
    }

    /// The folded needle when it is pure ASCII, for callers that can compare raw bytes
    /// without decoding them first (`strings` over a 523 MiB archive).
    pub fn ascii_needle(&self) -> Option<&str> {
        self.folded.as_deref().filter(|needle| needle.is_ascii())
    }

    pub fn matches(&self, haystack: &str) -> bool {
        match &self.folded {
            Some(needle) => haystack.to_lowercase().contains(needle.as_str()),
            None => haystack.contains(self.needle.as_str()),
        }
    }
}

/// Whether two labels are the same, under the same rule.
pub fn equals(left: &str, right: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        left == right
    } else {
        left.to_lowercase() == right.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_folds_case_and_sensitive_mode_does_not() {
        assert!(Contains::new("LIZ", false).matches("LizIZ"));
        assert!(!Contains::new("LIZ", true).matches("LizIZ"));
        assert!(Contains::new("LIZ", true).matches("xLIZy"));
        assert!(equals("ToJson", "tojson", false));
        assert!(!equals("ToJson", "tojson", true));
    }
}
