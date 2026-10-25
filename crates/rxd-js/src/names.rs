//! One own-label filter for JS listings, with exact names kept separate from substrings.
use crate::strings::JsString;
use rxd_core::{Error, Result};

#[derive(Clone, Default)]
pub struct NameQuery {
    pub name: Option<String>,
    pub filter: Option<String>,
    pub case_sensitive: bool,
}
impl NameQuery {
    pub fn validate(&self) -> Result<()> {
        if self.name.as_ref().is_some_and(String::is_empty) {
            return Err(Error::new("INVALID_QUERY", "name must not be empty"));
        }
        Ok(())
    }
    pub(crate) fn compile(&self) -> Names<'_> {
        Names {
            name: self.name.as_deref(),
            needle: self.filter.as_ref().map(|s| {
                if self.case_sensitive {
                    s.encode_utf16().collect()
                } else {
                    s.to_lowercase().encode_utf16().collect()
                }
            }),
            case_sensitive: self.case_sensitive,
        }
    }
}
/// Bound rows before allocation, independently of the final serialized-byte limit.
pub(crate) struct RecordBudget {
    records: usize,
    bytes: usize,
    limits: rxd_core::Limits,
}
impl RecordBudget {
    pub fn new(limits: rxd_core::Limits) -> Self {
        Self {
            records: 0,
            bytes: 0,
            limits,
        }
    }
    pub fn charge(&mut self, label: &JsString) -> Result<()> {
        let budget = self.limits;
        self.records += 1;
        self.bytes = self
            .bytes
            .saturating_add(label.len_utf16().saturating_mul(2) + 512);
        if budget.work.is_some_and(|limit| self.records > limit)
            || self.bytes > budget.metadata_bytes
        {
            return Err(Error::new(
                "RESOURCE_LIMIT",
                "JS name listing budget exceeded",
            ));
        }
        Ok(())
    }
}

pub(crate) struct Names<'a> {
    name: Option<&'a str>,
    needle: Option<Vec<u16>>,
    case_sensitive: bool,
}
impl Names<'_> {
    pub fn matches(&self, label: &JsString) -> bool {
        self.name.is_none_or(|name| label.equals_str(name))
            && self
                .needle
                .as_ref()
                .is_none_or(|needle| label.contains_prepared(needle, self.case_sensitive))
    }
}
