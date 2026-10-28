use crate::{
    Abc, Coordinate, Method, Record, analysis, ast,
    ir::{self, Op},
    reader::{invalid, resource},
    structure,
};
use rxd_core::{Error, Result, output::Output};
use std::{collections::HashSet, io::Write};

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
    PrivateProperties,
    RuntimeBindings,
    JavaScriptIntrinsics,
    AssumedBranchSemantics,
    SendableSemantics,
    LazyModuleTiming,
    AsyncFrameApproximation,
    ExceptionTypeSemantics,
    CoroutineAliasing,
    FrameConventions,
    IteratorOperations,
    SendableLocalModule,
    VersionSemantics,
    WholeAbcModules,
    ModuleNameEncoding,
    ModuleNameSnapshot,
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
            Self::PrivateProperties => "ARK_PRIVATE_PROPERTIES",
            Self::RuntimeBindings => "ARK_RUNTIME_BINDINGS",
            Self::JavaScriptIntrinsics => "ARK_JS_INTRINSICS",
            Self::AssumedBranchSemantics => "ARK_BRANCH_SEMANTICS",
            Self::SendableSemantics => "ARK_SENDABLE_SEMANTICS",
            Self::LazyModuleTiming => "ARK_LAZY_MODULE_TIMING",
            Self::AsyncFrameApproximation => "ARK_ASYNC_FRAME",
            Self::ExceptionTypeSemantics => "ARK_EXCEPTION_TYPES",
            Self::CoroutineAliasing => "ARK_COROUTINE_ALIASING",
            Self::FrameConventions => "ARK_FRAME_CONVENTIONS",
            Self::IteratorOperations => "ARK_ITERATOR_OPERATIONS",
            Self::SendableLocalModule => "ARK_SENDABLE_MODULE_TIMING",
            Self::VersionSemantics => "ARK_VERSION_SEMANTICS",
            Self::WholeAbcModules => "ARK_WHOLE_ABC_MODULES",
            Self::ModuleNameEncoding => "ARK_MODULE_NAME_ENCODING",
            Self::ModuleNameSnapshot => "ARK_MODULE_NAME_SNAPSHOT",
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
            Self::PrivateProperties => {
                "private fields use hidden per-object storage; Ark proxy unwrapping, inherited brands and AOT symbol identity may differ"
            }
            Self::RuntimeBindings => {
                "runtime bindings use explicit context records and globalThis fallback; host lexical globals and patch lifecycle may differ"
            }
            Self::JavaScriptIntrinsics => {
                "operation uses JavaScript intrinsics; Ark VM-specific behavior, reflection and modified globals may differ"
            }
            Self::AssumedBranchSemantics => {
                "comparison follows the opcode spelling; the pinned interpreter does not implement this branch variant"
            }
            Self::SendableSemantics => {
                "sendable state uses one JS realm; field layouts are retained but VM sharing, type enforcement, lexical class lookup and task scheduling are approximate"
            }
            Self::LazyModuleTiming => {
                "lazy/sendable module access is reconstructed as an eager ordinary JS import; initialization timing and sharing may differ"
            }
            Self::AsyncFrameApproximation => {
                "noncanonical async protocol uses explicit resume/promise state and JavaScript scheduling; VM promise identity and protocol-edge behavior may differ"
            }
            Self::ExceptionTypeSemantics => {
                "typed catches follow the pinned dynamic interpreter's first encoded handler; no static-language type filter is inferred"
            }
            Self::CoroutineAliasing => {
                "multiple generator identities share one reconstructed continuation; independent VM frame snapshots are NOT recovered"
            }
            Self::FrameConventions => {
                "unclassified function kind uses the encoded call-type argument frame and an ordinary JS callable; VM callable-kind restrictions may differ"
            }
            Self::IteratorOperations => {
                "iterator operations use JavaScript protocols; pending-exception precedence, async-from-sync edge cases and modified globals may differ"
            }
            Self::SendableLocalModule => {
                "sendable local module access uses the current JS context; Ark may initialize another module lazily or share it across realms"
            }
            Self::VersionSemantics => {
                "ABC version is outside the reference runtime's declared dynamic ISA compatibility range; decoded operations may have different semantics"
            }
            Self::WholeAbcModules => {
                "whole ABC output isolates records and namespaces their exports; original cross-module linkage, live bindings, and evaluation order are not reconstructed"
            }
            Self::ModuleNameEncoding => {
                "non-scalar module names use record-scoped bindings and UTF-16 encoded export aliases; original module linkage may differ"
            }
            Self::ModuleNameSnapshot => {
                "non-scalar module name is accessed as a namespace property snapshot; live-binding timing and missing-export diagnostics may differ"
            }
        }
    }
}

pub struct Reconstruction {
    pub source: Vec<u8>,
    pub stats: ReconstructionStats,
    pub warnings: Vec<ReconstructionWarning>,
}

impl ReconstructionStats {
    fn add(&mut self, other: Self) -> Result<()> {
        macro_rules! add {
            ($field:ident) => {
                self.$field = self
                    .$field
                    .checked_add(other.$field)
                    .ok_or_else(|| resource("Ark reconstruction statistics overflow"))?;
            };
        }
        add!(functions);
        add!(values);
        add!(eliminated_values);
        add!(blocks);
        add!(residual_blocks);
        add!(structured_tries);
        Ok(())
    }
}

fn module_names_need_scope(module: &crate::Module) -> bool {
    module
        .regular_imports
        .iter()
        .chain(&module.namespace_imports)
        .any(|import| {
            import
                .import_name
                .as_ref()
                .is_some_and(|name| name.scalar_text().is_none())
        })
        || module
            .local_exports
            .iter()
            .any(|export| export.export_name.scalar_text().is_none())
        || module.indirect_exports.iter().any(|export| {
            export.local_name.scalar_text().is_none() || export.export_name.scalar_text().is_none()
        })
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
        self.prepare(main, true, None, None)
    }

    pub fn reconstruct(&self, method: &Method) -> Result<Reconstruction> {
        self.prepare(method, false, None, None)
    }

    pub fn decompile_all(&self) -> Result<Reconstruction> {
        let mut output = Output::new(self.budget.output_bytes);
        let mut stats = ReconstructionStats::default();
        let mut warnings = Vec::new();
        let mut records = Vec::new();
        for offset in self.record_offsets() {
            let record = self.record(offset)?;
            if self
                .members(&record)?
                .methods
                .iter()
                .any(|method| method.code_offset.is_some())
            {
                let count = records
                    .len()
                    .checked_add(1)
                    .ok_or_else(|| resource("Ark record count overflow"))?;
                self.check_storage::<u32>(count)?;
                records
                    .try_reserve(1)
                    .map_err(|_| resource("Ark record list allocation failed"))?;
                records.push(offset);
            }
        }
        if records.is_empty() {
            return Err(Error::new("NOT_FOUND", "ABC has no code-bearing methods"));
        }
        let multiple = records.len() > 1;
        for offset in records {
            let record = self.record(offset)?;
            let members = self.members(&record)?;
            let first = members
                .methods
                .iter()
                .find(|method| method.code_offset.is_some())
                .ok_or_else(|| invalid("Ark code-bearing record changed during reconstruction"))?;
            let main = if self.module(&record)?.is_some() {
                members.methods.iter().find(|method| {
                    method.code_offset.is_some() && method.name.text == "func_main_0"
                })
            } else {
                None
            };
            let reconstructed = self.prepare(
                main.unwrap_or(first),
                main.is_some(),
                Some(&members.methods),
                multiple.then_some(offset),
            )?;
            writeln!(&mut output, "// record:0x{offset:x}")?;
            output.write_all(&reconstructed.source)?;
            stats.add(reconstructed.stats)?;
            for warning in reconstructed.warnings {
                if !warnings.contains(&warning) {
                    warnings
                        .try_reserve(1)
                        .map_err(|_| resource("Ark warning allocation failed"))?;
                    warnings.push(warning);
                }
            }
        }
        Ok(Reconstruction {
            source: output.into_bytes(),
            stats,
            warnings,
        })
    }

    fn prepare(
        &self,
        method: &Method,
        record: bool,
        all_methods: Option<&[Method]>,
        whole_record: Option<u32>,
    ) -> Result<Reconstruction> {
        let method = self.method(method.offset)?;
        let mut queue = vec![method.clone()];
        if let Some(methods) = all_methods {
            self.check_storage::<Method>(methods.len())?;
            queue
                .try_reserve(methods.len())
                .map_err(|_| resource("Ark definition queue allocation failed"))?;
            queue.extend(
                methods
                    .iter()
                    .filter(|candidate| {
                        candidate.code_offset.is_some() && candidate.offset != method.offset
                    })
                    .cloned(),
            );
        }
        let mut seen = HashSet::new();
        let mut constructors = HashSet::new();
        let mut units = Vec::new();
        let mut stats = ReconstructionStats::default();
        let module = self.module(&self.record(method.record)?)?;
        let names_need_scope = module.as_deref().is_some_and(module_names_need_scope);
        let scope = whole_record
            .or(names_need_scope.then_some(method.record))
            .map(|record| ast::RecordScope {
                record,
                whole_abc: whole_record.is_some(),
                non_scalar_names: names_need_scope,
            });
        while let Some(current) = queue.pop() {
            if seen.contains(&current.offset) {
                continue;
            }
            seen.try_reserve(1)
                .map_err(|_| resource("Ark definition set allocation failed"))?;
            seen.insert(current.offset);
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
                        constructors
                            .try_reserve(1)
                            .map_err(|_| resource("Ark constructor set allocation failed"))?;
                        constructors.insert(*constructor);
                        std::iter::once(*constructor)
                            .chain(members.iter().filter_map(|member| member.method))
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
            stats.add(ReconstructionStats {
                functions: 1,
                values: function.values.len(),
                eliminated_values: values.needed.iter().filter(|needed| !**needed).count(),
                blocks: body.owned.iter().filter(|owned| **owned).count(),
                residual_blocks: body.residual_blocks,
                structured_tries: body.structured_tries,
            })?;
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
            scope,
            self.version > [24, 0, 0, 0]
                || self.version < [0, 0, 0, 2]
                || matches!(self.version, [11, 0, 0, 0] | [11, 0, 1, 0]),
        )?;
        Ok(Reconstruction {
            source,
            stats,
            warnings,
        })
    }
}
