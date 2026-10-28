use crate::{
    Abc, Coordinate, Method, Record, analysis, ast,
    ir::{self, Op},
    reader::{invalid, resource},
    structure,
};
use rxd_core::{Error, Result};
use std::collections::HashSet;

#[derive(Default)]
pub struct ReconstructionStats {
    pub functions: usize,
    pub values: usize,
    pub eliminated_values: usize,
    pub blocks: usize,
    pub residual_blocks: usize,
    pub structured_tries: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconstructionWarning {
    IteratorRuntimeSemantics,
}

impl ReconstructionWarning {
    pub const fn code(self) -> &'static str {
        match self {
            Self::IteratorRuntimeSemantics => "ARK_ITERATOR_SEMANTICS",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::IteratorRuntimeSemantics => {
                "getiterator uses standard JavaScript semantics; Ark runtime edge cases or modified globals may differ"
            }
        }
    }
}

pub struct Reconstruction {
    pub source: Vec<u8>,
    pub stats: ReconstructionStats,
    pub warnings: &'static [ReconstructionWarning],
}

pub(crate) fn method_context(offset: u32, mut error: Error) -> Error {
    error.message = format!("method:0x{offset:x}: {}", error.message);
    error
}

impl Abc<'_> {
    pub fn decompile_target(&self, target: Coordinate) -> Result<Reconstruction> {
        match target {
            Coordinate::Method(offset) => self.decompile_method(&self.method(offset)?),
            Coordinate::Record(offset) => self.decompile_record(&self.record(offset)?),
        }
    }

    pub fn decompile_method(&self, method: &Method) -> Result<Reconstruction> {
        self.reconstruct(method)
    }

    pub fn decompile_record(&self, record: &Record) -> Result<Reconstruction> {
        let record = self.record(record.offset)?;
        let members = self.members(&record)?;
        let main = members
            .methods
            .iter()
            .find(|method| method.name.text == "func_main_0")
            .ok_or_else(|| invalid("Ark source record has no module initializer"))?;
        self.prepare(main, true)
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
                return Err(method_context(
                    current.offset,
                    invalid("Ark nested definition crosses records"),
                ));
            }
            let context = |error| method_context(current.offset, error);
            let mut function = ir::lift::lift(self, &current).map_err(context)?;
            let values = analysis::Values::analyze(&mut function, self.budget).map_err(context)?;
            let control = analysis::control(&function).map_err(context)?;
            let body =
                structure::structure(&function, &control, &values, self.budget).map_err(context)?;
            for (index, value) in function.values.iter().enumerate() {
                if values.needed[index]
                    && let Op::Closure { method, .. } = value.op
                {
                    queue
                        .try_reserve(1)
                        .map_err(|_| resource("Ark definition queue allocation failed"))?;
                    queue.push(self.method(method).map_err(context)?);
                }
            }
            stats.functions += 1;
            stats.values += function.values.len();
            stats.eliminated_values += values.needed.iter().filter(|needed| !**needed).count();
            stats.blocks += body.owned.iter().filter(|owned| **owned).count();
            stats.residual_blocks += body.residual_blocks;
            stats.structured_tries += body.structured_tries;
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
        let (source, uses_standard_iterator) = ast::lower(
            &units,
            module.as_deref(),
            self.budget,
            record.then_some(method.offset),
        )?;
        let warnings = if uses_standard_iterator {
            &[ReconstructionWarning::IteratorRuntimeSemantics][..]
        } else {
            &[]
        };
        Ok(Reconstruction {
            source,
            stats,
            warnings,
        })
    }
}
