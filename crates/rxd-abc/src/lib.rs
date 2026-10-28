//! Dynamic ABC metadata and target-scoped code access over one immutable source.
mod async_plan;
mod class_plan;
mod data;
mod dominators;
mod fresh;
mod instructions;
mod optimize;
mod provenance;
mod reader;
mod reconstruct;
mod structure;
pub use data::{Export, Import, Literal, LiteralValue, Module};
pub use instructions::{Decoded, EntityKind, Instruction, Opcode, Operand, OperandType, Parameter};

pub use reader::Name;
use reader::{Reader, invalid, range, resource, table, word};
use rxd_core::{Budget, BudgetPolicy, Error, Result, perf::Perf};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    mem::size_of,
    path::Path,
    sync::Arc,
};

const HEADER: usize = 60;
const VERSION: [u8; 4] = [12, 0, 6, 0];

pub struct Input {
    pub bytes: Vec<u8>,
    pub entry: Option<String>,
    pub budget: Budget,
}

impl Input {
    pub fn read(
        path: &Path,
        entry: Option<&str>,
        policy: BudgetPolicy,
        perf: &mut Perf,
    ) -> Result<Self> {
        let outer = policy.for_file(path)?;
        let bytes = perf.measure("read", || rxd_core::read_bytes(path, outer.input_bytes))?;
        let (bytes, entry) = if bytes.starts_with(b"PK") {
            let selected =
                rxd_zip::read_unique_entry_from_bytes(bytes, entry, ".abc", outer, perf)?;
            (selected.bytes, Some(selected.path))
        } else {
            if entry.is_some() {
                return Err(Error::new(
                    "INVALID_QUERY",
                    "--entry requires a HAP/ZIP container",
                ));
            }
            (bytes, None)
        };
        let budget = match policy {
            BudgetPolicy::InputScaled => Budget::for_input_bytes(bytes.len())?,
            BudgetPolicy::Fixed(budget) => budget,
        };
        Ok(Self {
            bytes,
            entry,
            budget,
        })
    }
}

#[derive(Clone, Copy)]
struct Index<'a> {
    bytes: &'a [u8],
}

impl<'a> Index<'a> {
    fn new(data: &'a [u8], offset: u32, count: u32) -> Result<Self> {
        if count > 0 && offset < HEADER as u32 {
            return Err(invalid("ABC index overlaps the header"));
        }
        Ok(Self {
            bytes: table(data, offset, count, 4)?,
        })
    }
    fn len(self) -> usize {
        self.bytes.len() / 4
    }
    fn get(self, index: u32) -> Result<u32> {
        if index as usize >= self.len() {
            return Err(invalid("ABC index operand is out of range"));
        }
        Ok(word(self.bytes, index as usize))
    }
    fn iter(self) -> impl Iterator<Item = u32> + 'a {
        self.bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_le_bytes(*bytes))
    }
}

struct Region<'a> {
    start: u32,
    end: u32,
    classes: Index<'a>,
    entities: Index<'a>,
}

pub struct Abc<'a> {
    data: &'a [u8],
    budget: Budget,
    regions: Vec<Region<'a>>,
    records: Index<'a>,
    literal_index: Index<'a>,
    sorted_records: Vec<u32>,
    foreign_start: u32,
    foreign_end: u32,
    names: RefCell<HashMap<u32, Arc<Name>>>,
    member_cache: RefCell<HashMap<u32, Arc<Members>>>,
    literal_cache: RefCell<HashMap<u32, Arc<Vec<Literal>>>>,
    module_cache: RefCell<HashMap<u32, Arc<Module>>>,
    metadata_bytes: Cell<usize>,
    decoded_instructions: Cell<usize>,
    decode_work: Cell<usize>,
    pub source_id: String,
}

#[derive(Debug)]
pub struct Record {
    pub offset: u32,
    pub name: Arc<Name>,
    pub fields: u32,
    pub methods: u32,
    pub flags: u32,
    pub foreign: bool,
    pub source_file: Option<u32>,
    pub source_lang: Option<u8>,
    members: u32,
    end: u32,
}

#[derive(Debug)]
pub struct Field {
    pub offset: u32,
    pub name: Arc<Name>,
    pub type_id: u32,
    pub flags: u32,
    pub value: Option<FieldValue>,
    end: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum FieldValue {
    Integer(i32),
    Bits(u32),
}

#[derive(Debug, Clone)]
pub struct Method {
    pub offset: u32,
    pub record: u32,
    pub name: Arc<Name>,
    pub flags: u32,
    pub code_offset: Option<u32>,
    pub debug_offset: Option<u32>,
    pub source_lang: Option<u8>,
    end: u32,
}

impl Field {
    pub fn byte_range(&self) -> std::ops::Range<u32> {
        self.offset..self.end
    }
}

impl Method {
    pub fn byte_range(&self) -> std::ops::Range<u32> {
        self.offset..self.end
    }
}

pub struct Members {
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    method_index: HashMap<u32, usize>,
}

#[derive(Debug)]
pub struct Catch {
    pub type_index: u32,
    pub pc: u32,
    pub length: u32,
}

#[derive(Debug)]
pub struct Try {
    pub pc: u32,
    pub length: u32,
    pub catches: Vec<Catch>,
}

pub struct Code<'a> {
    pub offset: u32,
    pub instruction_offset: u32,
    pub vregs: u32,
    pub args: u32,
    pub bytes: &'a [u8],
    pub tries: Vec<Try>,
}

#[derive(Debug, Clone, Copy)]
pub enum Coordinate {
    Record(u32),
    Method(u32),
}

impl Coordinate {
    pub fn parse(at: &str) -> Result<Self> {
        let (kind, value) = at.split_once(":0x").ok_or_else(|| {
            Error::new(
                "INVALID_QUERY",
                "expected record:0xOFFSET or method:0xOFFSET",
            )
        })?;
        let offset = u32::from_str_radix(value, 16)
            .map_err(|_| Error::new("INVALID_QUERY", "invalid ABC coordinate"))?;
        if format!("{kind}:0x{offset:x}") != at {
            return Err(Error::new(
                "INVALID_QUERY",
                "ABC coordinates must use the printed canonical spelling",
            ));
        }
        match kind {
            "record" => Ok(Self::Record(offset)),
            "method" => Ok(Self::Method(offset)),
            _ => Err(Error::new(
                "INVALID_QUERY",
                "expected record:0xOFFSET or method:0xOFFSET",
            )),
        }
    }
}

impl<'a> Abc<'a> {
    pub fn parse(data: &'a [u8], budget: Budget) -> Result<Self> {
        if data.len() > budget.input_bytes || data.len() > u32::MAX as usize {
            return Err(resource("ABC input exceeds its admitted byte length"));
        }
        if data.len() < HEADER || &data[..8] != b"PANDA\0\0\0" {
            return Err(invalid("expected a complete PANDA bytecode header"));
        }
        if data[12..16] != VERSION {
            return Err(Error::new(
                "UNSUPPORTED_INPUT",
                format!(
                    "unsupported dynamic ABC version {}.{}.{}.{}; supported: 12.0.6.0",
                    data[12], data[13], data[14], data[15]
                ),
            ));
        }
        if word(data, 4) as usize != data.len() {
            return Err(invalid("ABC file_size differs from input length"));
        }
        if word(data, 2) != adler2::adler32_slice(&data[12..]) {
            return Err(invalid("ABC checksum mismatch"));
        }
        let foreign_start = word(data, 5);
        let foreign_size = word(data, 6);
        range(data, foreign_start, foreign_size as usize)?;
        if foreign_size > 0 && foreign_start < HEADER as u32 {
            return Err(invalid("ABC foreign region overlaps header"));
        }
        let foreign_end = foreign_start
            .checked_add(foreign_size)
            .ok_or_else(|| invalid("ABC foreign region overflow"))?;
        let records = Index::new(data, word(data, 8), word(data, 7))?;
        for (count, offset) in [
            (word(data, 9), word(data, 10)),
            (word(data, 11), word(data, 12)),
        ] {
            let index = Index::new(data, offset, count)?;
            for target in index.iter() {
                validate_offset(data, target)?;
            }
        }
        let count = word(data, 13);
        if count as usize > budget.entries || records.len() > budget.entries {
            return Err(resource("ABC index count exceeds metadata admission"));
        }
        let region_bytes = table(data, word(data, 14), count, 40)?;
        let allocation = (count as usize)
            .checked_mul(size_of::<Region<'_>>())
            .and_then(|bytes| {
                records
                    .len()
                    .checked_mul(4)
                    .and_then(|n| bytes.checked_add(n))
            })
            .filter(|bytes| *bytes <= budget.metadata_bytes)
            .ok_or_else(|| resource("ABC indexes exceed metadata budget"))?;
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(count as usize)
            .map_err(|_| resource("ABC region allocation failed"))?;
        let mut previous_end = HEADER as u32;
        for bytes in region_bytes.as_chunks::<40>().0 {
            let start = word(bytes, 0);
            let end = word(bytes, 1);
            if start < previous_end || end <= start || end as usize > data.len() {
                return Err(invalid(
                    "ABC regions must be ordered, non-overlapping and inside input",
                ));
            }
            previous_end = end;
            let classes = Index::new(data, word(bytes, 3), word(bytes, 2))?;
            let entities = Index::new(data, word(bytes, 5), word(bytes, 4))?;
            if classes.len() > 65536 || entities.len() > 65536 {
                return Err(invalid("ABC region index exceeds its 16-bit format"));
            }
            for target in classes.iter() {
                if target >= HEADER as u32 {
                    validate_offset(data, target)?;
                } else if target > 15 {
                    return Err(invalid("invalid primitive class encoding"));
                }
            }
            for target in entities.iter() {
                validate_offset(data, target)?;
            }
            if [
                word(bytes, 6),
                word(bytes, 7),
                word(bytes, 8),
                word(bytes, 9),
            ] != [u32::MAX; 4]
            {
                return Err(invalid(
                    "dynamic ABC 12.0.6.0 requires disabled field/proto region indexes",
                ));
            }
            regions.push(Region {
                start,
                end,
                classes,
                entities,
            });
        }
        let mut sorted_records = Vec::new();
        sorted_records
            .try_reserve_exact(records.len())
            .map_err(|_| resource("ABC record allocation failed"))?;
        sorted_records.extend(records.iter());
        sorted_records.sort_unstable();
        if sorted_records.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("duplicate global ABC record offset"));
        }
        let literal_index = Index::new(data, word(data, 12), word(data, 11))?;
        let result = Self {
            data,
            literal_index,
            budget,
            regions,
            records,
            sorted_records,
            foreign_start,
            foreign_end,
            names: RefCell::new(HashMap::new()),
            member_cache: RefCell::new(HashMap::new()),
            literal_cache: RefCell::new(HashMap::new()),
            module_cache: RefCell::new(HashMap::new()),
            metadata_bytes: Cell::new(allocation),
            decoded_instructions: Cell::new(0),
            decode_work: Cell::new(0),
            source_id: format!("{:x}", Sha256::digest(data)),
        };
        for offset in result.records.iter() {
            validate_offset(data, offset)?;
            let region = result.region(offset)?;
            if !region.classes.iter().any(|target| target == offset) {
                return Err(invalid(
                    "global ABC record is absent from its region class index",
                ));
            }
        }
        Ok(result)
    }

    fn region(&self, offset: u32) -> Result<&Region<'a>> {
        let upper = self
            .regions
            .partition_point(|region| region.start <= offset);
        upper
            .checked_sub(1)
            .and_then(|index| self.regions.get(index))
            .filter(|region| offset < region.end)
            .ok_or_else(|| invalid(format!("ABC entity {offset:#x} is outside index regions")))
    }

    fn check_storage<T>(&self, count: usize) -> Result<()> {
        count
            .checked_mul(size_of::<T>())
            .and_then(|bytes| self.metadata_bytes.get().checked_add(bytes))
            .filter(|bytes| *bytes <= self.budget.metadata_bytes)
            .ok_or_else(|| resource("ABC metadata allocation exceeds budget"))?;
        if count > self.budget.entries {
            return Err(resource("ABC entity count exceeds metadata admission"));
        }
        Ok(())
    }

    fn charge_bytes(&self, bytes: usize) -> Result<()> {
        let total = self
            .metadata_bytes
            .get()
            .checked_add(bytes)
            .filter(|total| *total <= self.budget.metadata_bytes)
            .ok_or_else(|| resource("ABC metadata cache exceeds byte budget"))?;
        self.metadata_bytes.set(total);
        Ok(())
    }

    fn charge_storage<T>(&self, count: usize) -> Result<()> {
        self.check_storage::<T>(count)?;
        self.charge_bytes(count * size_of::<T>())
    }

    pub fn name(&self, offset: u32) -> Result<Arc<Name>> {
        if let Some(name) = self.names.borrow().get(&offset) {
            return Ok(Arc::clone(name));
        }
        validate_offset(self.data, offset)?;
        let available = self
            .budget
            .metadata_bytes
            .saturating_sub(self.metadata_bytes.get());
        let mut reader = Reader::new(self.data, offset, self.data.len())?;
        let name = Name::read(&mut reader, available)?;
        let size = name
            .units
            .len()
            .checked_mul(5)
            .and_then(|size| size.checked_add(size_of::<Name>() + 64))
            .and_then(|size| self.metadata_bytes.get().checked_add(size))
            .filter(|size| *size <= self.budget.metadata_bytes)
            .ok_or_else(|| resource("ABC string cache exceeds metadata budget"))?;
        let name = Arc::new(name);
        let mut names = self.names.borrow_mut();
        names
            .try_reserve(1)
            .map_err(|_| resource("ABC string index allocation failed"))?;
        names.insert(offset, Arc::clone(&name));
        self.metadata_bytes.set(size);
        Ok(name)
    }

    pub fn literal_offsets(&self) -> impl Iterator<Item = u32> + '_ {
        self.literal_index.iter()
    }

    pub fn record_offsets(&self) -> impl Iterator<Item = u32> + '_ {
        self.records.iter()
    }

    pub fn record(&self, offset: u32) -> Result<Record> {
        let index = self.sorted_records.binary_search(&offset).map_err(|_| {
            Error::new(
                "NOT_FOUND",
                format!("record:0x{offset:x} is not a record in this ABC"),
            )
        })?;
        let region = self.region(offset)?;
        let end = self
            .sorted_records
            .get(index + 1)
            .copied()
            .unwrap_or(region.end)
            .min(region.end);
        let name = self.name(offset)?;
        let foreign = (self.foreign_start..self.foreign_end).contains(&offset);
        if name.end > end {
            return Err(invalid("ABC record name overlaps the next record"));
        }
        let mut reader = Reader::new(self.data, name.end, end as usize)?;
        if foreign {
            return Ok(Record {
                offset,
                name,
                fields: 0,
                methods: 0,
                flags: 0,
                foreign,
                source_file: None,
                source_lang: None,
                members: reader.position as u32,
                end,
            });
        }
        let super_class = reader.u32()?;
        if super_class != 0 {
            validate_offset(self.data, super_class)?;
        }
        let flags = reader.uleb()?;
        let fields = reader.uleb()?;
        let methods = reader.uleb()?;
        let mut source_file = None;
        let mut source_lang = None;
        let mut previous = 0;
        loop {
            let tag = reader.u8()?;
            if tag == 0 {
                break;
            }
            if tag < previous || tag == previous && matches!(tag, 1 | 2 | 7) {
                return Err(invalid("invalid or repeated ABC class tag"));
            }
            previous = tag;
            match tag {
                1 => {
                    let count = reader.uleb()?;
                    for _ in 0..count {
                        let target = region.classes.get(u32::from(reader.u16()?))?;
                        validate_offset(self.data, target)?;
                    }
                }
                2 => {
                    source_lang = Some(reader.u8()?);
                }
                3..=6 => {
                    validate_offset(self.data, reader.u32()?)?;
                }
                7 => {
                    let offset = reader.u32()?;
                    self.name(offset)?;
                    source_file = Some(offset);
                }
                _ => return Err(invalid("unknown ABC class tag")),
            }
        }
        let minimum = (fields as usize)
            .checked_mul(10)
            .and_then(|size| {
                (methods as usize)
                    .checked_mul(10)
                    .and_then(|n| size.checked_add(n))
            })
            .ok_or_else(|| invalid("ABC member count overflow"))?;
        if minimum > reader.remaining() {
            return Err(invalid("ABC members cannot fit inside the record range"));
        }
        Ok(Record {
            offset,
            name,
            fields,
            methods,
            flags,
            foreign,
            source_file,
            source_lang,
            members: reader.position as u32,
            end,
        })
    }

    pub fn members(&self, record: &Record) -> Result<Arc<Members>> {
        if let Some(members) = self.member_cache.borrow().get(&record.offset) {
            return Ok(Arc::clone(members));
        }
        self.charge_storage::<Field>(record.fields as usize)?;
        self.charge_storage::<Method>(record.methods as usize)?;
        self.charge_bytes(size_of::<Members>() + 64)?;
        let mut reader = Reader::new(self.data, record.members, record.end as usize)?;
        let mut fields = Vec::new();
        fields
            .try_reserve_exact(record.fields as usize)
            .map_err(|_| resource("ABC field allocation failed"))?;
        for _ in 0..record.fields {
            fields.push(self.field_at(&mut reader, record.offset)?);
        }
        let mut methods = Vec::new();
        methods
            .try_reserve_exact(record.methods as usize)
            .map_err(|_| resource("ABC method allocation failed"))?;
        for _ in 0..record.methods {
            methods.push(self.method_at(&mut reader, record.offset)?);
        }
        let mut method_index = HashMap::new();
        method_index
            .try_reserve(methods.len())
            .map_err(|_| resource("ABC method index allocation failed"))?;
        for (index, method) in methods.iter().enumerate() {
            method_index.insert(method.offset, index);
        }
        self.charge_bytes(
            methods
                .len()
                .checked_mul(size_of::<u32>() + size_of::<usize>() + 16)
                .ok_or_else(|| resource("ABC method index size overflow"))?,
        )?;
        let members = Arc::new(Members {
            fields,
            methods,
            method_index,
        });
        let mut cache = self.member_cache.borrow_mut();
        cache
            .try_reserve(1)
            .map_err(|_| resource("ABC member cache allocation failed"))?;
        cache.insert(record.offset, Arc::clone(&members));
        Ok(members)
    }

    fn field_at(&self, reader: &mut Reader<'a>, owner: u32) -> Result<Field> {
        let offset = reader.position as u32;
        let region = self.region(offset)?;
        if region.classes.get(u32::from(reader.u16()?))? != owner {
            return Err(invalid("ABC field owner differs from its record"));
        }
        let type_id = region.classes.get(u32::from(reader.u16()?))?;
        let name = self.name(reader.u32()?)?;
        let flags = reader.uleb()?;
        let mut value = None;
        let mut previous = 0;
        loop {
            let tag = reader.u8()?;
            if tag == 0 {
                break;
            }
            if tag < previous || tag <= 2 && value.is_some() {
                return Err(invalid("invalid or repeated ABC field tag"));
            }
            previous = tag;
            match tag {
                1 => value = Some(FieldValue::Integer(reader.sleb()?)),
                2 => value = Some(FieldValue::Bits(reader.u32()?)),
                3..=6 => {
                    validate_offset(self.data, reader.u32()?)?;
                }
                _ => return Err(invalid("unknown ABC field tag")),
            }
        }
        Ok(Field {
            offset,
            name,
            type_id,
            flags,
            value,
            end: reader.position as u32,
        })
    }

    fn method_at(&self, reader: &mut Reader<'a>, owner: u32) -> Result<Method> {
        let offset = reader.position as u32;
        let region = self.region(offset)?;
        if region.classes.get(u32::from(reader.u16()?))? != owner {
            return Err(invalid("ABC method owner differs from its record"));
        }
        if reader.u16()? != u16::MAX {
            return Err(invalid("dynamic ABC method has a prototype index"));
        }
        let name = self.name(reader.u32()?)?;
        let flags = reader.uleb()?;
        if (flags >> 16) as usize >= self.regions.len() {
            return Err(invalid("ABC method index-header number is out of range"));
        }
        let mut code_offset = None;
        let mut debug_offset = None;
        let mut source_lang = None;
        let mut previous = 0;
        loop {
            let tag = reader.u8()?;
            if tag == 0 {
                break;
            }
            if tag < previous || tag == previous && matches!(tag, 1 | 2 | 4 | 5 | 7) {
                return Err(invalid("invalid or repeated ABC method tag"));
            }
            previous = tag;
            match tag {
                1 => {
                    let offset = reader.u32()?;
                    validate_offset(self.data, offset)?;
                    code_offset = Some(offset);
                }
                2 => {
                    source_lang = Some(reader.u8()?);
                }
                5 => {
                    let offset = reader.u32()?;
                    validate_offset(self.data, offset)?;
                    debug_offset = Some(offset);
                }
                3 | 4 | 6..=9 => {
                    validate_offset(self.data, reader.u32()?)?;
                }
                _ => return Err(invalid("unknown ABC method tag")),
            }
        }
        Ok(Method {
            offset,
            record: owner,
            name,
            flags,
            code_offset,
            debug_offset,
            source_lang,
            end: reader.position as u32,
        })
    }

    pub fn method(&self, offset: u32) -> Result<Method> {
        let missing = || {
            Error::new(
                "NOT_FOUND",
                format!("method:0x{offset:x} is not a method in this ABC"),
            )
        };
        let region = self.region(offset).map_err(|_| missing())?;
        let bytes = range(self.data, offset, 2).map_err(|_| missing())?;
        let index = u16::from_le_bytes(bytes.try_into().unwrap());
        let owner = region
            .classes
            .get(u32::from(index))
            .map_err(|_| missing())?;
        if self.sorted_records.binary_search(&owner).is_err() {
            return Err(missing());
        }
        let record = self.record(owner)?;
        let members = self.members(&record)?;
        members
            .method_index
            .get(&offset)
            .and_then(|index| members.methods.get(*index))
            .cloned()
            .ok_or_else(missing)
    }

    pub fn method_call_type(&self, method: &Method) -> Result<u8> {
        let start = method
            .offset
            .checked_add(8)
            .ok_or_else(|| invalid("ABC method offset overflow"))?;
        let mut reader = Reader::new(self.data, start, method.end as usize)?;
        reader.uleb()?;
        let mut call_type = u32::MAX;
        loop {
            let tag = reader.u8()?;
            if tag == 0 {
                break;
            }
            if tag == 2 {
                reader.u8()?;
                continue;
            }
            let offset = reader.u32()?;
            if tag != 6 {
                continue;
            }
            let mut annotation = Reader::new(self.data, offset, self.data.len())?;
            let index = u32::from(annotation.u16()?);
            let count = usize::from(annotation.u16()?);
            let class = self.region(offset)?.classes.get(index)?;
            if self.name(class)?.scalar_text() != Some("L_ESCallTypeAnnotation;") {
                continue;
            }
            let length = count
                .checked_mul(9)
                .ok_or_else(|| invalid("ABC annotation element count overflow"))?;
            let elements = annotation.take(length)?;
            for i in 0..count {
                let position = i * 8;
                let name = u32::from_le_bytes(elements[position..position + 4].try_into().unwrap());
                if self.name(name)?.scalar_text() != Some("callType") {
                    continue;
                }
                if !matches!(elements[count * 8 + i], b'6' | b'7') {
                    return Err(invalid("ABC call-type annotation is not an integer"));
                }
                call_type =
                    u32::from_le_bytes(elements[position + 4..position + 8].try_into().unwrap());
            }
        }
        Ok((call_type & 15) as u8)
    }

    pub fn resolve_entity(&self, method: &Method, index: u32) -> Result<u32> {
        self.regions
            .get((method.flags >> 16) as usize)
            .ok_or_else(|| invalid("ABC method index-header number is out of range"))?
            .entities
            .get(index)
    }

    pub fn code(&self, method: &Method) -> Result<Code<'a>> {
        let offset = method
            .code_offset
            .ok_or_else(|| Error::new("NOT_FOUND", "method has no code item"))?;
        let mut reader = Reader::new(self.data, offset, self.data.len())?;
        let vregs = reader.uleb()?;
        let args = reader.uleb()?;
        vregs
            .checked_add(args)
            .ok_or_else(|| invalid("ABC register count overflow"))?;
        let size = reader.uleb()?;
        let tries_count = reader.uleb()?;
        let instruction_offset = reader.position as u32;
        let bytes = reader.take(size as usize)?;
        if tries_count as usize > reader.remaining() / 6 {
            return Err(invalid("truncated ABC try table"));
        }
        self.check_storage::<Try>(tries_count as usize)?;
        let mut tries = Vec::new();
        tries
            .try_reserve_exact(tries_count as usize)
            .map_err(|_| resource("ABC try table allocation failed"))?;
        for _ in 0..tries_count {
            let pc = reader.uleb()?;
            let length = reader.uleb()?;
            validate_pc_range(pc, length, size)?;
            let count = reader.uleb()?;
            if count as usize > reader.remaining() / 3 {
                return Err(invalid("truncated ABC catch table"));
            }
            self.check_storage::<Catch>(count as usize)?;
            let mut catches = Vec::new();
            catches
                .try_reserve_exact(count as usize)
                .map_err(|_| resource("ABC catch allocation failed"))?;
            for index in 0..count {
                let type_index = reader.uleb()?;
                let handler_pc = reader.uleb()?;
                let handler_size = reader.uleb()?;
                validate_pc_range(handler_pc, handler_size, size)?;
                if type_index > 0 {
                    self.region(method.offset)?.classes.get(type_index - 1)?;
                } else if index + 1 != count {
                    return Err(invalid("ABC catch-all handler must be last"));
                }
                catches.push(Catch {
                    type_index,
                    pc: handler_pc,
                    length: handler_size,
                });
            }
            tries.push(Try {
                pc,
                length,
                catches,
            });
        }
        Ok(Code {
            offset,
            instruction_offset,
            vregs,
            args,
            bytes,
            tries,
        })
    }
}

fn validate_offset(data: &[u8], offset: u32) -> Result<()> {
    if offset as usize <= HEADER || offset as usize >= data.len() {
        return Err(invalid(format!("invalid ABC entity offset {offset:#x}")));
    }
    Ok(())
}

fn validate_pc_range(pc: u32, length: u32, size: u32) -> Result<()> {
    if pc.checked_add(length).is_none_or(|end| end > size) {
        return Err(invalid("ABC exception range exceeds its code item"));
    }
    Ok(())
}
