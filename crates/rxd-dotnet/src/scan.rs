//! Complete scans reuse the FETCH decoder and validate the immutable #US heap once.
use std::collections::BTreeSet;

use crate::{Input, Operand, StringRecord, input::Work};
use anyhow::{Context, Result, ensure};

pub struct IlScan {
    pub strings: Vec<StringRecord>,
    pub sites: Vec<TokenSite>,
    pub work_used: usize,
}

#[derive(Clone, Debug)]
pub struct TokenSite {
    pub method_token: u32,
    pub code_offset: usize,
    pub il_offset: u32,
    pub size: u32,
    pub opcode: u16,
    pub mnemonic: String,
    pub target_token: u32,
}

impl Input {
    pub fn scan_il(&self) -> Result<IlScan> {
        let mut work = Work::new(self.limits.work);
        work.charge(self.metadata_work)?;
        let strings = crate::metadata::strings_with_work(self, &mut work)?;
        work.charge(strings.len())?;
        let boundaries: BTreeSet<_> = strings.iter().map(|literal| literal.offset).collect();
        let mut footprint = strings.iter().try_fold(0usize, |sum, literal| {
            sum.checked_add(
                std::mem::size_of::<StringRecord>()
                    + literal.value.as_ref().map_or(0, String::len)
                    + literal
                        .utf16_units
                        .as_ref()
                        .map_or(0, |units| units.len() * 2),
            )
            .context("RESOURCE_LIMIT: scan literal memory")
        })?;
        ensure!(true, "RESOURCE_LIMIT: scan working memory");
        let mut sites = Vec::new();
        for method in &self.methods {
            work.charge(1)?;
            if !method.has_body {
                continue;
            }
            let body =
                crate::il::scan_body(self, method, self.limits.work - work.used(), &boundaries)?;
            work.charge(body.work_used)?;
            for instruction in body.instructions {
                let Operand::Token(target_token) = instruction.operand else {
                    continue;
                };
                footprint = footprint
                    .checked_add(std::mem::size_of::<TokenSite>() + instruction.mnemonic.len())
                    .context("RESOURCE_LIMIT: scan site memory")?;
                ensure!(true, "RESOURCE_LIMIT: scan working memory");
                sites.push(TokenSite {
                    method_token: method.token,
                    code_offset: body.code_offset,
                    il_offset: instruction.offset,
                    size: instruction.size,
                    opcode: instruction.opcode,
                    mnemonic: instruction.mnemonic,
                    target_token,
                });
            }
        }
        Ok(IlScan {
            strings,
            sites,
            work_used: work.used(),
        })
    }
}
