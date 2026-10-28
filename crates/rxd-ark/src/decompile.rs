use crate::{
    Abc, Coordinate, Method, Record, analysis, ast,
    ir::{self, Op},
    reader::{invalid, resource},
    structure,
};
use rxd_core::Result;
use std::collections::HashSet;

#[derive(Default)]
pub struct ReconstructionStats {
    pub functions: usize,
    pub values: usize,
    pub eliminated_values: usize,
    pub blocks: usize,
    pub residual_blocks: usize,
}

pub struct Reconstruction {
    pub source: Vec<u8>,
    pub stats: ReconstructionStats,
}

impl Abc<'_> {
    pub fn decompile_target(&self, target: Coordinate) -> Result<Vec<u8>> {
        match target {
            Coordinate::Method(offset) => self.decompile_method(&self.method(offset)?),
            Coordinate::Record(offset) => self.decompile_record(&self.record(offset)?),
        }
    }

    pub fn decompile_method(&self, method: &Method) -> Result<Vec<u8>> {
        Ok(self.reconstruct(method)?.source)
    }

    pub fn decompile_record(&self, record: &Record) -> Result<Vec<u8>> {
        let record = self.record(record.offset)?;
        let members = self.members(&record)?;
        let main = members
            .methods
            .iter()
            .find(|method| method.name.text == "func_main_0")
            .ok_or_else(|| invalid("Ark source record has no module initializer"))?;
        Ok(self.prepare(main, true)?.source)
    }

    pub fn reconstruct(&self, method: &Method) -> Result<Reconstruction> {
        self.prepare(method, false)
    }

    fn prepare(&self, method: &Method, record: bool) -> Result<Reconstruction> {
        let method = self.method(method.offset)?;
        let mut queue = vec![method.clone()];
        let mut seen = HashSet::new();
        let mut units = Vec::new();
        let mut stats = ReconstructionStats::default();
        let module = self.module(&self.record(method.record)?)?;
        while let Some(current) = queue.pop() {
            if !seen.insert(current.offset) {
                continue;
            }
            if current.record != method.record {
                return Err(invalid("Ark nested definition crosses records"));
            }
            let mut function = ir::lift::lift(self, &current).map_err(|mut error| {
                error.message = format!("method:0x{:x}: {}", current.offset, error.message);
                error
            })?;
            let values = analysis::Values::analyze(&mut function, self.budget)?;
            let control = analysis::control(&function)?;
            let body = structure::structure(&function, &control, &values, self.budget)?;
            for (index, value) in function.values.iter().enumerate() {
                if values.needed[index]
                    && let Op::Closure { method, .. } = value.op
                {
                    queue
                        .try_reserve(1)
                        .map_err(|_| resource("Ark definition queue allocation failed"))?;
                    queue.push(self.method(method)?);
                }
            }
            stats.functions += 1;
            stats.values += function.values.len();
            stats.eliminated_values += values.needed.iter().filter(|needed| !**needed).count();
            stats.blocks += body.owned.iter().filter(|owned| **owned).count();
            stats.residual_blocks += body.residual_blocks;
            units
                .try_reserve(1)
                .map_err(|_| resource("Ark function plan allocation failed"))?;
            units.push(ast::Prepared {
                function,
                values,
                body,
            });
        }
        units.sort_by_key(|unit| unit.function.method.offset);
        let source = ast::lower(
            &units,
            module.as_deref(),
            self.budget,
            record.then_some(method.offset),
        )?;
        Ok(Reconstruction { source, stats })
    }
}
