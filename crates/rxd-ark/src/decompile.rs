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
    PropertyEnumeration,
    ArraySpread,
    SpreadCall,
    ClassReconstruction,
    GeneratorReconstruction,
    ControlFlowReconstruction,
}

impl ReconstructionWarning {
    pub const fn code(self) -> &'static str {
        match self {
            Self::IteratorRuntimeSemantics => "ARK_ITERATOR_SEMANTICS",
            Self::PropertyEnumeration => "ARK_PROPERTY_ENUMERATION",
            Self::ArraySpread => "ARK_ARRAY_SPREAD",
            Self::SpreadCall => "ARK_SPREAD_CALL",
            Self::ClassReconstruction => "ARK_CLASS_RECONSTRUCTION",
            Self::GeneratorReconstruction => "ARK_GENERATOR_RECONSTRUCTION",
            Self::ControlFlowReconstruction => "ARK_CONTROL_FLOW_RECONSTRUCTION",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::IteratorRuntimeSemantics => {
                "getiterator uses standard JavaScript semantics; Ark runtime edge cases or modified globals may differ"
            }
            Self::PropertyEnumeration => {
                "property enumeration uses a suspended JavaScript for-in; Ark snapshot timing, proxies and mutation edge cases may differ"
            }
            Self::ArraySpread => {
                "array spread preserves UTF-16 strings and result descriptors; non-string iterator selection uses JavaScript rather than Ark internal fast paths"
            }
            Self::SpreadCall => {
                "spread arguments use JavaScript iteration; Ark array fast paths and modified globals may differ"
            }
            Self::ClassReconstruction => {
                "class behavior uses explicit constructor/home-object contexts; method constructibility/names, external class markers and VM error details may differ"
            }
            Self::GeneratorReconstruction => {
                "generator protocol uses an explicit resume adapter; VM identity, cross-generator method borrowing, reflection and modified globals may differ"
            }
            Self::ControlFlowReconstruction => {
                "original source structure was not recovered here; decoded branches and handlers are represented by state dispatch"
            }
        }
    }
}

pub struct Reconstruction {
    pub source: Vec<u8>,
    pub stats: ReconstructionStats,
    pub warnings: Vec<ReconstructionWarning>,
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
        let mut constructors = HashSet::new();
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
                if !values.needed[index] {
                    continue;
                }
                let dependencies = match &value.op {
                    Op::Closure { method, .. } | Op::Method { method, .. } => vec![*method],
                    Op::Class {
                        constructor,
                        members,
                        ..
                    } => {
                        constructors.insert(*constructor);
                        std::iter::once(*constructor)
                            .chain(members.iter().map(|member| member.method))
                            .collect()
                    }
                    _ => vec![],
                };
                queue
                    .try_reserve(dependencies.len())
                    .map_err(|_| resource("Ark definition queue allocation failed"))?;
                for method in dependencies {
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
        for unit in &mut units {
            unit.function.frame.constructor |= constructors.contains(&unit.function.method.offset);
        }
        units.sort_by_key(|unit| unit.function.method.offset);
        let (source, warnings) = ast::lower(
            &units,
            module.as_deref(),
            self.budget,
            record.then_some(method.offset),
        )?;
        Ok(Reconstruction {
            source,
            stats,
            warnings,
        })
    }
}
