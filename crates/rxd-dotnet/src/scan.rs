//! Complete scans reuse the FETCH decoder and validate the immutable #US heap once.
use std::collections::BTreeSet;

use crate::{
    Input, Operand, StringRecord,
    input::{Work, WorkingSet},
};
use anyhow::Result;

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
        let mut work = Work::new(self.limits.work, self.limits.metadata_bytes);
        work.charge(self.metadata_work)?;
        let strings = crate::metadata::strings_with_work(self, &mut work)?;
        work.charge(strings.len())?;
        let boundaries: BTreeSet<_> = strings.iter().map(|literal| literal.offset).collect();
        let mut footprint = WorkingSet::new(self.limits.metadata_bytes, "scan working set");
        for literal in &strings {
            footprint.add(
                std::mem::size_of::<StringRecord>()
                    + literal.value.as_ref().map_or(0, String::len)
                    + literal
                        .utf16_units
                        .as_ref()
                        .map_or(0, |units| units.len() * 2),
            )?;
        }
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
                footprint.add(std::mem::size_of::<TokenSite>() + instruction.mnemonic.len())?;
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
