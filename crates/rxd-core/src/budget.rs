//! Input-scaled CLI bounds, separate from explicit caller/service quotas.
//!
//! The constant allowances preserve existing small-input behavior. They are not total
//! CLI ceilings: encoded input contributes metadata/index storage allowances.
//! Ordinary CLI has no work, instruction, entry-byte or output-byte quota;
//! allocation can still fail.
//! Amplification/recursion guards still fail explicitly; this is not an unlimited mode.
use std::path::Path;

use crate::{Budget, Error, Failure, Result};

#[derive(Clone, Copy, Debug)]
pub enum BudgetPolicy {
    InputScaled,
    Fixed(Budget),
}

impl BudgetPolicy {
    /// Size admission is not an identity check. Parsers still read/hash their fixed buffer.
    /// A file that grows past the admitted length is refused by the bounded reader.
    pub fn for_file(self, path: &Path) -> Result<Budget> {
        if let Self::Fixed(budget) = self {
            return Ok(budget);
        }
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(Error::failure(
                Failure::Input,
                "expected a regular local file",
            ));
        }
        let bytes = usize::try_from(metadata.len())
            .map_err(|_| Error::failure(Failure::Resource, "input length is not addressable"))?;
        Budget::for_input_bytes(bytes)
    }
}

impl Budget {
    /// Bound metadata/index storage relative to admitted input. Work, instructions,
    /// individual entry bytes and serialization have no quota (`None`), never a
    /// numeric unlimited sentinel. All derived arithmetic is checked.
    ///
    /// These are safety envelopes, not proofs of semantic completeness or RSS ceilings.
    /// Formats with compressed/shared structures keep their local expansion guards too.
    pub fn for_input_bytes(bytes: usize) -> Result<Self> {
        let base = Self::default();
        let grow = |allowance: usize, factor: usize| {
            bytes
                .checked_mul(factor)
                .and_then(|n| allowance.checked_add(n))
                .filter(|n| *n <= isize::MAX as usize)
                .ok_or_else(|| {
                    Error::failure(Failure::Resource, "input-derived resource bound overflow")
                })
        };
        Ok(Self {
            input_bytes: bytes,
            entry_bytes: None,
            metadata_bytes: grow(base.metadata_bytes, 64)?,
            output_bytes: None,
            entries: grow(base.entries, 1)?,
            instructions: None,
            work: None,
            ast_depth: base.ast_depth,
            semantic_nodes: grow(base.semantic_nodes, 4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_bounds_preserve_guards_and_refuse_arithmetic_overflow() {
        let fixed = Budget::default();
        let scaled = Budget::for_input_bytes(1024).unwrap();
        assert_eq!(scaled.input_bytes, 1024);
        assert_eq!(scaled.work, None);
        assert_eq!(scaled.instructions, None);
        assert_eq!(scaled.entry_bytes, None);
        assert!(scaled.metadata_bytes > fixed.metadata_bytes);
        assert_eq!(scaled.output_bytes, None);
        assert!(fixed.output_bytes.is_some());
        assert_eq!(scaled.ast_depth, fixed.ast_depth);
        assert_eq!(
            Budget::for_input_bytes(usize::MAX).unwrap_err().kind(),
            Some(Failure::Resource)
        );
    }
}
