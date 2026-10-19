//! Selected native hook facts from the linked engine's structured IR. No C parsing.
use kuna_analysis::pass::{AnalysisCtx, AnalysisPass};
use kuna_console::engine::{bootstrap_from_object_with_isa, EntryLookupError, EntrySelector};
use kuna_decomp::{dtype::type_metatype, fspec::ProtoParameter};
use object::{Object, ObjectSection, ObjectSegment};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum Scalar {
    Signed,
    Unsigned,
    Pointer,
    Void,
    Unknown,
}
#[derive(Clone, Debug)]
pub struct Value {
    pub name: String,
    pub c_type: String,
    pub scalar: Scalar,
    pub bytes: i32,
    pub register: String,
    pub stack_offset: Option<u64>,
    pub indirect: bool,
}
#[derive(Clone, Debug)]
pub struct Signature {
    pub provenance: &'static str,
    pub convention: String,
    pub variadic: bool,
    pub parameters: Vec<Value>,
    pub result: Value,
}
pub struct Selection {
    pub address: u64,
    pub signature: Option<Signature>,
}
#[derive(Debug)]
pub enum Failure {
    Invalid(String),
    Missing,
    Ambiguous,
    Budget,
    Timeout,
    Analysis(String),
}

/// The snapshot path MUST refer to a private capture, never the caller's original path.
/// Bootstrap/discovery are cooperative: checked work/expansion ceilings precede the
/// calls, elapsed checks follow them. Only the selected drive has an internal deadline.
pub fn inspect(
    bytes: &[u8],
    snapshot: &Path,
    specs: &Path,
    target: EntryTarget<'_>,
    typed: bool,
    steps: u64,
    expanded: usize,
    timeout: Duration,
) -> Result<Selection, Failure> {
    // Implicit external facts are outside the fixed capture and have no caller
    // expansion budget. Do not let inherited CLI overrides open those files.
    for variable in ["kuna_pdb_path", "kuna_fid_db"] {
        if std::env::var_os(variable).is_some_and(|value| !value.is_empty()) {
            return Err(Failure::Invalid(format!("hook analysis does not accept inherited {variable}; external metadata is outside the capture")));
        }
    }
    let start = Instant::now();
    let check_time = || {
        if start.elapsed() >= timeout {
            Err(Failure::Timeout)
        } else {
            Ok(())
        }
    };
    let file = object::File::parse(bytes).map_err(|e| Failure::Invalid(e.to_string()))?;
    let mut work = bytes.len() as u64;
    let mut expansion = bytes.len() as u64;
    for section in file.sections() {
        // Read declared decompressed sizes BEFORE upstream can allocate/decompress.
        let declared = section
            .compressed_file_range()
            .map_err(|e| Failure::Invalid(e.to_string()))?;
        let size = declared.uncompressed_size.max(section.size());
        expansion = expansion.checked_add(size).ok_or(Failure::Budget)?;
        work = work.checked_add(size).ok_or(Failure::Budget)?;
    }
    for segment in file.segments() {
        // __PAGEZERO is an inaccessible address reservation, not mapped bytes
        // or executable discovery work. Actual zero-filled mappings still count.
        if matches!(
            segment.flags(),
            object::SegmentFlags::MachO { initprot: 0, .. }
        ) && segment.file_range().1 == 0
        {
            continue;
        }
        work = work.checked_add(segment.size()).ok_or(Failure::Budget)?;
        expansion = expansion
            .checked_add(segment.size())
            .ok_or(Failure::Budget)?;
    }
    if expansion > expanded as u64 || work > steps {
        return Err(Failure::Budget);
    }
    check_time()?;
    let specs = specs
        .to_str()
        .ok_or_else(|| Failure::Invalid("non-UTF8 specs path".into()))?;
    let path = snapshot
        .to_str()
        .ok_or_else(|| Failure::Invalid("non-UTF8 snapshot path".into()))?;
    let mut program = bootstrap_from_object_with_isa(path, "", &[specs.to_owned()], None)
        .map_err(|e| Failure::Analysis(e.to_string()))?;
    check_time()?;
    // Listing is needed only when cheap loader/metadata entries do not recognize
    // the request. Never seed the requested address as a function declaration.
    let candidate = match target {
        EntryTarget::Address(address) => program.find_entry_at(address),
        EntryTarget::Name(name) => program.find_entry_by_name(name),
    };
    // General name lookup also accepts mapped sub_ADDRESS placeholders. Only
    // inventory membership proves an entry and permits skipping discovery.
    let known = candidate.is_some_and(|entry| {
        program
            .function_entries_executable()
            .iter()
            .any(|known| known.addr == entry.addr)
    });
    program.arch_mut().analysis_listing = !known;
    program
        .commit_pending_analysis()
        .map_err(|e| Failure::Analysis(e.to_string()))?;
    check_time()?;
    let entries = program.function_entries_executable();
    work = work
        .checked_add(entries.len() as u64)
        .ok_or(Failure::Budget)?;
    if work >= steps {
        return Err(Failure::Budget);
    }
    let entry = match target {
        EntryTarget::Address(address) => program.find_entry_at(address).ok_or(Failure::Missing)?,
        EntryTarget::Name(name) => program
            .resolve_entry(&EntrySelector::Name(name.to_owned()))
            .map_err(|e| {
                if matches!(e, EntryLookupError::Ambiguous { .. }) {
                    Failure::Ambiguous
                } else {
                    Failure::Missing
                }
            })?,
    };
    if !entries.iter().any(|known| known.addr == entry.addr) {
        return Err(Failure::Missing);
    }
    let static_address = program.output_address_offset(&entry.addr);
    // The loader also recognizes PE THUMB/ARMNT machines that object may leave
    // unknown. The resolved engine context, not object's generic enum, owns mode.
    let thumb = program
        .arch()
        .with_context_db_mut(|db| db.get_variable_value(b"TMode", &entry.addr).unwrap_or(0))
        != 0;
    let address = static_address | u64::from(thumb);
    if !typed {
        return Ok(Selection {
            address,
            signature: None,
        });
    }
    let image = kuna_analysis::loadimage_object::ObjectLoadImage::from_bytes_silent(path, bytes)
        .map_err(|e| Failure::Analysis(e.to_string()))?;
    let debug = kuna_analysis::dwarf::DwarfPass.run(&AnalysisCtx {
        file: &file,
        bytes,
        image: &image,
        arch: program.arch(),
        listing: None,
        image_path: None,
    });
    check_time()?;
    let debug_proto = debug
        .cpp_dwarf
        .prototypes
        .into_iter()
        .find(|(at, _)| *at == static_address)
        .map(|(_, p)| p)
        .or_else(|| {
            debug
                .symbols
                .iter()
                .find(|symbol| symbol.addr == static_address)
                .and_then(|symbol| debug.prototypes.into_iter().find(|p| p.name == symbol.name))
        });
    let mapped = program.dwarf_locals_for(static_address);
    let remaining = timeout
        .checked_sub(start.elapsed())
        .ok_or(Failure::Timeout)?;
    program.arch_mut().kuna_fn_budget = Some(remaining);
    program.arch_mut().max_instructions = u32::try_from(steps.saturating_sub(work))
        .unwrap_or(u32::MAX)
        .max(1);
    let declared = program.declared_extent(static_address);
    let fd = kuna_console::decompile_step::decompile_one(
        program.arch_mut(),
        &entry.name,
        entry.addr,
        declared,
        &kuna_console::decompile_step::DecompileSeed {
            mapped_symbols: &mapped,
            usepoint_symbols: &[],
            dynamic_symbols: &[],
            object_assertions: &[],
            pending_proto: debug_proto.as_ref(),
            flow_overrides: &[],
            mapped_params: &[],
        },
        &[],
    )
    .result
    .map_err(|e| {
        if start.elapsed() >= timeout {
            Failure::Timeout
        } else if e
            == kuna_base::error::KunaError::lowlevel("Flow exceeded maximum allowable instructions")
        {
            Failure::Budget
        } else {
            Failure::Analysis(e.to_string())
        }
    })?;
    check_time()?;
    let proto = fd.get_func_proto();
    if !proto.has_store() || !proto.has_model() {
        return Ok(Selection {
            address,
            signature: None,
        });
    }
    let value = |parameter: &dyn ProtoParameter| {
        let ty = parameter.get_type();
        let scalar = match ty.map(|t| t.get_metatype()) {
            Some(type_metatype::TYPE_INT | type_metatype::TYPE_ENUM_INT) => Scalar::Signed,
            Some(
                type_metatype::TYPE_UINT | type_metatype::TYPE_ENUM_UINT | type_metatype::TYPE_BOOL,
            ) => Scalar::Unsigned,
            Some(type_metatype::TYPE_PTR) => Scalar::Pointer,
            Some(type_metatype::TYPE_VOID) => Scalar::Void,
            _ => Scalar::Unknown,
        };
        let at = parameter.get_address();
        let space = at.get_space();
        let register = space
            .map(|space| {
                program.arch().translate().get_register_name(
                    space,
                    at.get_offset(),
                    parameter.get_size(),
                )
            })
            .unwrap_or_default();
        Value {
            name: parameter.get_name().to_owned(),
            c_type: ty
                .map(|t| kuna_decomp::printc::type_to_c_string(program.arch(), t))
                .unwrap_or_else(|| "unknown".into()),
            scalar,
            bytes: parameter.get_size(),
            register,
            stack_offset: space
                .filter(|s| s.get_name() == "stack")
                .map(|_| at.get_offset()),
            indirect: parameter.is_indirect_storage() || parameter.is_hidden_return(),
        }
    };
    let count = proto.num_params();
    if count < 0 || count as u64 > steps.saturating_sub(work) {
        return Err(Failure::Budget);
    }
    let parameters = (0..count)
        .filter_map(|i| proto.get_param(i))
        .map(value)
        .collect();
    let signature = Signature {
        provenance: if debug_proto.is_some() {
            "debug-derived"
        } else {
            "inferred by Kuna"
        },
        convention: proto.get_model_name().to_owned(),
        variadic: proto.is_dotdotdot(),
        parameters,
        result: value(proto.get_output()),
    };
    check_time()?;
    Ok(Selection {
        address,
        signature: Some(signature),
    })
}
#[derive(Clone, Copy)]
pub enum EntryTarget<'a> {
    Name(&'a str),
    Address(u64),
}
