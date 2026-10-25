//! Prefilter reference operands by target-index bytes, then check bitmap membership.

/// Reference indices occupy little-endian operands at pc + 2.
pub(super) struct Targets {
    /// One bit per index below [`Self::BITMAP_INDICES`], indexed by the index itself.
    bits: Vec<u64>,
    /// The (usually empty) tail of indices the bitmap does not cover.
    wide: Vec<u32>,
    /// `None` when the target set is too large for the filter to pay off.
    bytes: Option<TargetBytes>,
}

impl Targets {
    pub(super) const FIXED_BYTES: usize = Self::BITMAP_INDICES / 8;

    /// Above this many targets the filter scans the code once per target, which
    /// costs more than the instruction decode it would skip.
    const MAX_FILTER_TARGETS: usize = 4;

    /// All reference operands are 16-bit except const-string/jumbo.
    const BITMAP_INDICES: usize = 1 << 16;

    /// Whether the filter would let `code` through; `true` when filtering is off.
    pub(super) fn might_reference(&self, code: &[u8]) -> bool {
        match &self.bytes {
            Some(bytes) => bytes.matches(code),
            None => true,
        }
    }

    #[inline]
    pub(super) fn contains(&self, index: u32) -> bool {
        let index = index as usize;
        if index < Self::BITMAP_INDICES {
            self.bits[index >> 6] >> (index & 63) & 1 != 0
        } else {
            self.wide.binary_search(&(index as u32)).is_ok()
        }
    }

    pub(super) fn new(indices: impl IntoIterator<Item = u32>) -> Self {
        // Sort here; caller index order is not an invariant.
        let mut sorted: Vec<u32> = indices.into_iter().collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut bits = vec![0u64; Self::BITMAP_INDICES / 64];
        let mut wide = Vec::new();
        for &index in &sorted {
            if (index as usize) < Self::BITMAP_INDICES {
                bits[index as usize >> 6] |= 1u64 << (index & 63);
            } else {
                wide.push(index);
            }
        }
        let bytes = (sorted.len() <= Self::MAX_FILTER_TARGETS).then(|| TargetBytes::new(&sorted));
        Self { bits, wide, bytes }
    }

    /// The same targets, with the byte filter off (tests and ablations).
    #[cfg(test)]
    pub(super) fn unfiltered(indices: impl IntoIterator<Item = u32>) -> Self {
        let mut targets = Self::new(indices);
        targets.bytes = None;
        targets
    }
}

/// Encoded target indices, split by operand width.
struct TargetBytes {
    pairs: Vec<[u8; 2]>,
    quads: Vec<[u8; 4]>,
}

impl TargetBytes {
    fn new(indices: &[u32]) -> Self {
        let mut pairs = Vec::new();
        let mut quads = Vec::new();
        for &index in indices {
            if index <= u32::from(u16::MAX) {
                pairs.push((index as u16).to_le_bytes());
            } else {
                quads.push(index.to_le_bytes());
            }
        }
        Self { pairs, quads }
    }

    fn matches(&self, code: &[u8]) -> bool {
        self.pairs.iter().any(|pair| contains_pair(code, *pair))
            || self
                .quads
                .iter()
                .any(|quad| memchr::memmem::find(code, quad).is_some())
    }
}

/// Two-byte search: `memchr` on the first byte, then a check on both sides,
/// because the match may start one byte before the position found.
fn contains_pair(code: &[u8], [first, second]: [u8; 2]) -> bool {
    let mut from = 0;
    while let Some(found) = memchr::memchr(first, &code[from..]) {
        let at = from + found;
        if code.get(at + 1) == Some(&second) || (at > 0 && code[at - 1] == second) {
            return true;
        }
        from = at + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dex::tests::const_string_fixture;
    use crate::dex::{Dex, PARALLEL_SCAN_CLASSES};
    use crate::query::{Pattern, Query};

    #[test]
    fn pair_filter_finds_matches_at_both_edges() {
        assert!(contains_pair(b"\x1a\x05", [0x1a, 0x05]));
        assert!(contains_pair(b"\x00\x1a\x05\x00", [0x1a, 0x05]));
        assert!(contains_pair(b"\x05\x1a", [0x1a, 0x05]));
        assert!(!contains_pair(b"\x1a\x00\x05", [0x1a, 0x05]));
        assert!(!contains_pair(b"", [0x1a, 0x05]));
        assert!(!contains_pair(b"\x1a", [0x1a, 0x05]));
    }

    #[test]
    fn bitmap_membership_matches_the_target_list() {
        let targets = Targets::new([0u32, 1, 63, 64, 65, 1000, 65535, 65536, 70000]);
        for &index in &[0u32, 1, 63, 64, 65, 1000, 65535, 65536, 70000] {
            assert!(targets.contains(index), "{index} must be a target");
        }
        for &index in &[2u32, 62, 66, 999, 1001, 65534, 65537, 65538, 12345] {
            assert!(!targets.contains(index), "{index} must not be a target");
        }
        // Indices outside the bitmap land in the sorted tail, which is searched.
        let wide = Targets::new([65536u32, 70000, 65537]);
        assert!(wide.contains(65537) && wide.contains(70000) && !wide.contains(65538));
        assert_eq!(wide.wide, [65536, 65537, 70000]);
        // Duplicates and unsorted input must not disturb either test.
        let messy = Targets::new([5u32, 3, 5, 3, 70000, 70000]);
        assert!(messy.contains(3) && messy.contains(5) && messy.contains(70000));
        assert!(!messy.contains(4));
    }

    #[test]
    fn byte_filter_matches_the_unfiltered_scan() {
        let data = const_string_fixture(PARALLEL_SCAN_CLASSES);
        let dex = Dex::parse(&data).unwrap();
        let (kind, resolved) = dex
            .resolve_targets(&Query::String(Pattern::search("Authorization", false)))
            .unwrap();
        let targets = Targets::new(resolved.clone());
        assert!(targets.bytes.is_some(), "one target must stay filterable");
        let filtered = dex.scan_all_classes(kind, &targets).unwrap();
        let unfiltered = Targets::unfiltered(resolved);
        assert_eq!(filtered, dex.scan_all_classes(kind, &unfiltered).unwrap());
        assert_eq!(filtered.len(), PARALLEL_SCAN_CLASSES);
    }
}
