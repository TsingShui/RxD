use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};

use crate::{Input, StringRecord, StringReference, StringUse, StringUses, input::Work};

struct Scan {
    literals: Vec<StringRecord>,
    sites: Vec<StringReference>,
    work_used: usize,
}

fn scan(input: &mut Input<'_>) -> Result<Scan> {
    let crate::IlScan {
        strings: literals,
        mut sites,
        work_used,
    } = input.scan_il()?;
    sites.retain(|site| site.opcode == 0x72);
    Ok(Scan {
        literals,
        sites,
        work_used,
    })
}

pub(crate) fn uses(
    input: &mut Input<'_>,
    contains: Option<&str>,
    case_sensitive: bool,
) -> Result<StringUses> {
    let Scan {
        literals,
        sites,
        work_used,
    } = scan(input)?;
    let mut work = Work::new(
        input
            .limits
            .work
            .map(|limit| limit.saturating_sub(work_used)),
        input.limits.metadata_bytes,
    );
    let mut owners: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
    for site in sites {
        work.charge(1)?;
        owners
            .entry((site.target_token & 0xffffff, site.method_token))
            .or_default()
            .push(site.il_offset);
    }
    // Fold the UTF-16 needle once per query.
    let needle = contains
        .map(|text| {
            let text = if case_sensitive {
                text.to_owned()
            } else {
                text.to_lowercase()
            };
            Utf16Needle::new(&text, &mut work)
        })
        .transpose()?;
    let mut groups = Vec::new();
    for (literal_index, literal) in literals.iter().enumerate() {
        work.charge(1)?;
        if let Some(needle) = &needle {
            let folded;
            let units: &[u16] = if let Some(units) = &literal.utf16_units {
                units
            } else {
                folded = literal
                    .value
                    .as_deref()
                    .unwrap_or_default()
                    .encode_utf16()
                    .collect::<Vec<_>>();
                &folded
            };
            let matches = if case_sensitive {
                needle.matches(units.iter().copied(), &mut work)?
            } else {
                let lowered: Vec<u16> = String::from_utf16_lossy(units)
                    .to_lowercase()
                    .encode_utf16()
                    .collect();
                needle.matches(lowered.into_iter(), &mut work)?
            };
            if !matches {
                continue;
            }
        }
        let mut used = false;
        while let Some((key, _)) = owners
            .range((literal.offset, 0)..=(literal.offset, u32::MAX))
            .next()
        {
            let key = *key;
            let offsets = owners.remove(&key).expect("selected string owner");
            work.charge(1)?;
            groups.push(StringUse {
                literal_index,
                method_token: Some(key.1),
                il_offsets: offsets,
            });
            used = true;
        }
        if !used {
            groups.push(StringUse {
                literal_index,
                method_token: None,
                il_offsets: Vec::new(),
            });
        }
    }
    Ok(StringUses { literals, groups })
}

/// Match decoded literals exactly for anchors or by case-controlled substring.
pub(crate) fn references(
    input: &mut Input<'_>,
    pattern: &str,
    exact: bool,
    case_sensitive: bool,
) -> Result<Vec<StringReference>> {
    let Scan {
        literals,
        sites,
        work_used,
    } = scan(input)?;
    let mut work = Work::new(
        input
            .limits
            .work
            .map(|limit| limit.saturating_sub(work_used)),
        input.limits.metadata_bytes,
    );
    work.charge(pattern.len())?;
    work.charge(literals.len())?;
    let tokens: BTreeSet<_> = literals
        .iter()
        .filter(|literal| {
            literal.value.as_deref().is_some_and(|value| {
                if exact {
                    rxd_core::filter::equals(value, pattern, case_sensitive)
                } else {
                    rxd_core::filter::Contains::new(pattern, case_sensitive).matches(value)
                }
            })
        })
        .map(|literal| literal.offset)
        .collect();
    let mut matched = Vec::new();
    for site in sites {
        work.charge(1)?;
        if tokens.contains(&(site.target_token & 0xffffff)) {
            matched.push(site);
        }
    }
    Ok(matched)
}

pub(crate) fn token_references(input: &mut Input<'_>, token: u32) -> Result<Vec<StringReference>> {
    let Scan {
        literals,
        sites,
        work_used,
    } = scan(input)?;
    let mut work = Work::new(
        input
            .limits
            .work
            .map(|limit| limit.saturating_sub(work_used)),
        input.limits.metadata_bytes,
    );
    work.charge(literals.len())?;
    ensure!(
        token >> 24 == 0x70
            && literals
                .iter()
                .any(|literal| literal.offset == token & 0xffffff),
        "INVALID_QUERY: user-string token does not identify a heap entry"
    );
    let mut matched = Vec::new();
    for site in sites {
        work.charge(1)?;
        if site.target_token == token {
            matched.push(site);
        }
    }
    Ok(matched)
}

/// Linear UTF-16 matching retains isolated-surrogate neighbors without a lossy view.
struct Utf16Needle {
    units: Vec<u16>,
    prefix: Vec<usize>,
}

impl Utf16Needle {
    fn new(value: &str, work: &mut Work) -> Result<Self> {
        let count = value.encode_utf16().count();
        // The needle is the caller's own pattern; its working memory is charged as work,
        // not refused against an output ceiling.
        work.charge(count)?;
        let units: Vec<_> = value.encode_utf16().collect();
        let mut prefix = vec![0; count];
        let mut matched = 0;
        for index in 1..count {
            while matched > 0 && units[index] != units[matched] {
                work.charge(1)?;
                matched = prefix[matched - 1];
            }
            if units[index] == units[matched] {
                matched += 1;
            }
            prefix[index] = matched;
        }
        Ok(Self { units, prefix })
    }

    fn matches(&self, units: impl Iterator<Item = u16>, work: &mut Work) -> Result<bool> {
        if self.units.is_empty() {
            return Ok(true);
        }
        let mut matched = 0;
        for unit in units {
            work.charge(1)?;
            while matched > 0 && unit != self.units[matched] {
                work.charge(1)?;
                matched = self.prefix[matched - 1];
            }
            if unit == self.units[matched] {
                matched += 1;
            }
            if matched == self.units.len() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
