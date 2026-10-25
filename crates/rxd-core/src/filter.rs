//! Shared case-folded matching; sensitive mode compares exact case.

#[derive(Clone, Debug)]
pub struct Contains {
    needle: String,
    /// None means case-sensitive matching.
    folded: Option<String>,
}

impl Contains {
    pub fn new(needle: &str, case_sensitive: bool) -> Self {
        Self {
            needle: needle.to_owned(),
            folded: (!case_sensitive).then(|| needle.to_lowercase()),
        }
    }

    pub fn text(&self) -> &str {
        &self.needle
    }

    /// Folded ASCII needle for byte-level matching, otherwise None.
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
