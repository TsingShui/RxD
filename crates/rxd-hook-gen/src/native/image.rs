use super::{Request, RuntimeModule, Target};
use crate::{AnalysisBudget, Diagnostic, ErrorKind, Limits, Result, Stage};
use object::read::macho::MachHeader;
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol};
use std::collections::BTreeSet;

pub(super) struct Image {
    pub arch: &'static str,
    pub pointer_size: u8,
    pub platform: &'static str,
    pub base: u64,
    pub address: u64,
    pub thumb: bool,
    pub module: Option<String>,
    pub label: String,
    pub main_name: Option<String>,
    pub signature: Option<super::engine::Signature>,
    pub page_mappings: Vec<(u64, u64)>,
}
fn error(kind: ErrorKind, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(kind, Stage::Selection, message)
}

fn macho_main_offset<Mach: MachHeader>(
    file: &object::read::macho::MachOFile<'_, Mach>,
) -> Result<Option<u64>> {
    let mut commands = file
        .macho_header()
        .load_commands(file.endian(), file.data(), 0)
        .map_err(|e| error(ErrorKind::InvalidInput, e.to_string()))?;
    while let Some(command) = commands
        .next()
        .map_err(|e| error(ErrorKind::InvalidInput, e.to_string()))?
    {
        if let Some(entry) = command
            .entry_point()
            .map_err(|e| error(ErrorKind::InvalidInput, e.to_string()))?
        {
            return Ok(Some(entry.entryoff.get(file.endian())));
        }
    }
    Ok(None)
}

pub(super) fn resolve(request: &Request, limits: Limits) -> Result<Image> {
    let mut budget = AnalysisBudget::new(limits)?;
    let mut retained = 0usize;
    let mut reserve = |amount: usize| -> Result<()> {
        retained = retained.checked_add(amount).ok_or_else(|| {
            error(
                ErrorKind::ResourceLimit,
                "native index allocation estimate overflow",
            )
        })?;
        if retained > limits.expanded_bytes {
            return Err(error(
                ErrorKind::ResourceLimit,
                "native index expanded-byte budget exceeded",
            ));
        }
        Ok(())
    };
    let file = object::File::parse(request.input.bytes()).map_err(|e| {
        error(
            ErrorKind::InvalidInput,
            format!("invalid native image: {e}"),
        )
    })?;
    if !matches!(
        file.kind(),
        object::ObjectKind::Executable | object::ObjectKind::Dynamic
    ) {
        return Err(error(
            ErrorKind::Unsupported,
            "requires a linked executable or shared image; relocatable objects have no runtime mapping",
        ));
    }
    if !file.is_little_endian() {
        return Err(error(
            ErrorKind::Unsupported,
            "big-endian targets are not supported by this Frida provider",
        ));
    }
    if file.sub_architecture().is_some() {
        return Err(error(
            ErrorKind::Unsupported,
            "architecture subtypes (including arm64e/PAC) require an explicit supported mode",
        ));
    }
    let pe_machine = match &file {
        object::File::Pe32(file) => Some(
            file.nt_headers()
                .file_header
                .machine
                .get(object::LittleEndian),
        ),
        object::File::Pe64(file) => Some(
            file.nt_headers()
                .file_header
                .machine
                .get(object::LittleEndian),
        ),
        _ => None,
    };
    let pe_thumb = matches!(
        pe_machine,
        Some(object::pe::IMAGE_FILE_MACHINE_THUMB | object::pe::IMAGE_FILE_MACHINE_ARMNT)
    );
    // object deliberately leaves the older THUMB machine unknown; the pinned
    // Kuna loader supports it, and both THUMB/ARMNT establish Thumb code mode.
    let architecture = if pe_thumb {
        object::Architecture::Arm
    } else {
        file.architecture()
    };
    let (arch, pointer_size) = match architecture {
        object::Architecture::X86_64 => ("x64", 8),
        object::Architecture::I386 => ("ia32", 4),
        object::Architecture::Aarch64 => ("arm64", 8),
        object::Architecture::Arm => ("arm", 4),
        _ => {
            return Err(error(
                ErrorKind::Unsupported,
                "unsupported Frida architecture",
            ));
        }
    };
    if file.is_64() != (pointer_size == 8) {
        return Err(error(
            ErrorKind::Unsupported,
            "architecture and address width disagree",
        ));
    }
    if let object::FileFlags::Elf { os_abi, .. } = file.flags()
        && !matches!(os_abi, 0 | 3 | 97)
    {
        return Err(error(
            ErrorKind::Unsupported,
            "only Linux/System V ELF runtime mappings are supported",
        ));
    }
    let platform = match file.format() {
        object::BinaryFormat::Elf => "linux",
        object::BinaryFormat::Pe => "windows",
        object::BinaryFormat::MachO => "darwin",
        _ => {
            return Err(error(
                ErrorKind::Unsupported,
                "supported mappings are ELF PT_LOAD, PE image RVAs and thin Mach-O segments",
            ));
        }
    };
    let main_offset = match &file {
        object::File::MachO32(file) => macho_main_offset(file)?,
        object::File::MachO64(file) => macho_main_offset(file)?,
        _ => None,
    };
    let mut image_entry = if main_offset.is_none() && file.entry() != 0 {
        Some(file.entry() | u64::from(pe_thumb))
    } else {
        None
    };
    let mut mappings = Vec::new();
    let mut page_mappings = Vec::new();
    let mut bases = Vec::new();
    for segment in file.segments() {
        budget.consume(1)?;
        let (offset, length) = segment.file_range();
        let start = segment.address();
        let end = start
            .checked_add(segment.size())
            .ok_or_else(|| error(ErrorKind::InvalidInput, "mapping overflows address space"))?;
        let file_end = offset
            .checked_add(length)
            .ok_or_else(|| error(ErrorKind::InvalidInput, "mapping file range overflow"))?;
        if file_end > request.input.bytes().len() as u64 || length > segment.size() {
            return Err(error(ErrorKind::InvalidInput, "invalid mapping file range"));
        }
        if file.format() == object::BinaryFormat::Elf && (start % 4096 != offset % 4096) {
            return Err(error(
                ErrorKind::Unsupported,
                "ELF load mappings must have congruent page offsets",
            ));
        }
        if let Some(entry_offset) = main_offset
            && entry_offset >= offset
            && entry_offset < file_end
        {
            let address = start
                .checked_add(entry_offset - offset)
                .ok_or_else(|| error(ErrorKind::InvalidInput, "Mach-O entry mapping overflow"))?;
            if image_entry.replace(address).is_some() {
                return Err(error(
                    ErrorKind::Unsupported,
                    "Mach-O entry has overlapping file mappings",
                ));
            }
        }
        if file.format() == object::BinaryFormat::Elf && length > 0 {
            reserve(32)?;
            page_mappings.push((start, offset));
        }
        if offset == 0 && length > 0 {
            reserve(16)?;
            bases.push(start);
        }
        let executable = match segment.flags() {
            object::SegmentFlags::Elf { p_flags } => p_flags & object::elf::PF_X != 0,
            object::SegmentFlags::Coff { characteristics } => {
                characteristics & object::pe::IMAGE_SCN_MEM_EXECUTE != 0
            }
            object::SegmentFlags::MachO { initprot, .. } => {
                initprot & object::macho::VM_PROT_EXECUTE != 0
            }
            _ => false,
        };
        if executable {
            reserve(32)?;
            mappings.push((
                start,
                end,
                start
                    .checked_add(length)
                    .ok_or_else(|| error(ErrorKind::InvalidInput, "mapping overflow"))?,
            ));
        }
    }
    let base = if file.format() == object::BinaryFormat::Pe {
        file.relative_address_base()
    } else {
        // Requiring the mapped header removes page-size/load-bias ambiguity for
        // headerless mappings. Mach-O __PAGEZERO has no file bytes and is ignored.
        *bases.iter().min().ok_or_else(|| {
            error(
                ErrorKind::Unsupported,
                "image must map its file header at offset zero",
            )
        })?
    };
    if mappings.iter().any(|(start, _, _)| *start < base) {
        return Err(error(
            ErrorKind::Unsupported,
            "executable mapping precedes runtime module base",
        ));
    }
    let physical = |v: u64| if arch == "arm" { v & !1 } else { v };
    let executable = |v: u64| {
        mappings.iter().any(|(start, end, file_end)| {
            physical(v) >= *start && physical(v) < *end && physical(v) < *file_end
        })
    };
    if let Target::StaticAddress(address) = request.target
        && arch == "arm64"
        && address % 4 != 0
    {
        return Err(error(
            ErrorKind::Unsupported,
            "ARM64 function addresses must be four-byte aligned",
        ));
    }
    if let Target::StaticAddress(address) = request.target
        && !executable(address)
    {
        return Err(error(
            ErrorKind::NotFound,
            "static VMA is not file-backed executable memory (not a file offset)",
        ));
    }
    let mut arm_markers = Vec::new();
    if arch == "arm" {
        for symbol in file.symbols() {
            budget.consume(1)?;
            if let Ok(name) = symbol.name() {
                let kind = name.split('.').next().unwrap_or("");
                if matches!(kind, "$t" | "$a" | "$d") {
                    reserve(32)?;
                    arm_markers.push((physical(symbol.address()), kind));
                }
            }
        }
        arm_markers.sort_unstable();
    }
    let mut entries = BTreeSet::new();
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        budget.consume(1)?;
        if symbol.kind() != object::SymbolKind::Text
            || symbol.is_undefined()
            || !executable(symbol.address())
        {
            continue;
        }
        if file.format() == object::BinaryFormat::MachO {
            // N_SECT in a text section is not enough: __mh_execute_header is
            // tagged __text but names the mapped header, not a function body.
            let in_section = symbol
                .section_index()
                .and_then(|index| file.section_by_index(index).ok())
                .is_some_and(|section| {
                    let start = physical(section.address());
                    physical(symbol.address()) >= start
                        && section
                            .size()
                            .checked_add(start)
                            .is_some_and(|end| physical(symbol.address()) < end)
                });
            if !in_section {
                continue;
            }
        }
        if let Ok(name) = symbol.name() {
            limits.check_target(name)?;
            let mut address = symbol.address();
            if arch == "arm" {
                let macho_thumb = matches!(symbol.flags(), object::SymbolFlags::MachO { n_desc } if n_desc & object::macho::N_ARM_THUMB_DEF != 0);
                let marker = arm_markers
                    .partition_point(|(at, _)| *at <= physical(address))
                    .checked_sub(1)
                    .map(|i| arm_markers[i].1);
                if marker == Some("$d") {
                    continue;
                }
                if (address & 1 != 0 || macho_thumb) && marker == Some("$a") {
                    return Err(error(
                        ErrorKind::Unsupported,
                        "ARM function symbol and mapping marker disagree about instruction mode",
                    ));
                }
                if pe_thumb || macho_thumb || marker == Some("$t") {
                    address |= 1;
                }
            }
            reserve(name.len().saturating_add(128))?;
            entries.insert((address, name.to_owned()));
        }
    }
    if file.format() == object::BinaryFormat::Pe {
        for export in file
            .exports()
            .map_err(|e| error(ErrorKind::InvalidInput, e.to_string()))?
        {
            budget.consume(1)?;
            if executable(export.address()) {
                let name = std::str::from_utf8(export.name())
                    .map_err(|_| error(ErrorKind::Unsupported, "non-UTF8 export name"))?;
                limits.check_target(name)?;
                reserve(name.len().saturating_add(128))?;
                entries.insert((export.address() | u64::from(pe_thumb), name.to_owned()));
            }
        }
    }
    // The actual executable image entry is independent evidence for a stripped
    // entry hook; arbitrary mapped addresses are never declared into existence.
    if let Some(entry) = image_entry
        && executable(entry)
    {
        entries.insert((entry, "<image-entry>".into()));
    }
    let candidates: BTreeSet<_> = entries
        .iter()
        .filter(|(address, name)| match &request.target {
            Target::Name(want) => name == want,
            Target::StaticAddress(want) => physical(*want) == physical(*address),
        })
        .map(|(address, _)| *address)
        .collect();
    if candidates.len() > 1 {
        return Err(error(
            ErrorKind::Ambiguous,
            "target matches multiple function entries or instruction-set modes",
        ));
    }
    let (address, signature) =
        super::engine::inspect(request, candidates.first().copied(), limits)?;
    if !executable(address) {
        return Err(error(
            ErrorKind::NotFound,
            "target is not in file-backed executable memory",
        ));
    }
    if file.format() == object::BinaryFormat::MachO {
        let mut in_code = false;
        for section in file.sections() {
            budget.consume(1)?;
            if section.kind() == object::SectionKind::Text
                && physical(address) >= section.address()
                && section
                    .address()
                    .checked_add(section.size())
                    .is_some_and(|end| physical(address) < end)
            {
                in_code = true;
                break;
            }
        }
        if !in_code {
            return Err(error(
                ErrorKind::NotFound,
                "Mach-O target is not inside an executable code section",
            ));
        }
    }
    if let Target::StaticAddress(want) = request.target
        && arch == "arm"
        && want & 1 != 0
        && address & 1 == 0
    {
        return Err(error(
            ErrorKind::Unsupported,
            "Thumb selector conflicts with the recognized ARM entry",
        ));
    }
    let alignment = if arch == "arm64" || (arch == "arm" && address & 1 == 0) {
        4
    } else if arch == "arm" {
        2
    } else {
        1
    };
    if physical(address) % alignment != 0 || (pointer_size == 4 && address > u32::MAX as u64) {
        return Err(error(
            ErrorKind::Unsupported,
            "function entry violates architecture address alignment/width",
        ));
    }
    let label = entries
        .iter()
        .filter(|(at, _)| *at == address)
        .map(|(_, name)| name.as_str())
        .find(|name| *name != "<image-entry>")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("0x{address:x}"));
    let executable_image = file.kind() == object::ObjectKind::Executable
        || (file.format() == object::BinaryFormat::Elf && file.entry() != 0);
    let module = match &request.options.module {
        RuntimeModule::Main => None,
        RuntimeModule::Named(name) => {
            if name.is_empty() || name.contains('\0') {
                return Err(error(
                    ErrorKind::InvalidArguments,
                    "module name is empty or contains NUL",
                ));
            }
            Some(name.clone())
        }
        RuntimeModule::Auto if executable_image => None,
        RuntimeModule::Auto => {
            let name = request
                .input
                .name()
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("");
            if name.is_empty() || name.contains('\0') {
                return Err(error(
                    ErrorKind::InvalidArguments,
                    "cannot derive module identity; supply an explicit runtime module",
                ));
            }
            limits.check_target(name)?;
            Some(name.to_owned())
        }
    };
    let main_name = if matches!(request.options.module, RuntimeModule::Auto) && module.is_none() {
        let name = request
            .input
            .name()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        limits.check_target(name)?;
        if name.is_empty() || name.contains('\0') {
            return Err(error(
                ErrorKind::InvalidArguments,
                "cannot derive main-module name",
            ));
        }
        Some(name.to_owned())
    } else {
        None
    };
    budget.consume(0)?;
    Ok(Image {
        arch,
        pointer_size,
        platform,
        base,
        address: physical(address),
        thumb: arch == "arm" && address & 1 != 0,
        module,
        label,
        main_name,
        signature,
        page_mappings,
    })
}
