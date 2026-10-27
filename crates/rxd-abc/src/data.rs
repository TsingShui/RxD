use crate::{
    Abc, FieldValue, Name, Record,
    reader::{Reader, invalid, resource},
    validate_offset,
};
use rxd_core::{Error, Result};
use std::sync::Arc;

#[derive(Debug)]
pub enum LiteralValue {
    Byte(u8),
    Integer(u32),
    Float(u32),
    Double(u64),
    String(Arc<Name>),
    Method(u32),
    Array(u32),
    Affiliate(u16),
}

#[derive(Debug)]
pub struct Literal {
    pub tag: u8,
    pub value: LiteralValue,
}

#[derive(Debug)]
pub struct Import {
    pub local_name: Arc<Name>,
    pub import_name: Option<Arc<Name>>,
    pub request: u16,
}

#[derive(Debug)]
pub struct Export {
    pub local_name: Arc<Name>,
    pub export_name: Arc<Name>,
    pub request: Option<u16>,
}

#[derive(Debug)]
pub struct Module {
    pub offset: u32,
    pub declared_literals: u32,
    pub requests: Vec<Arc<Name>>,
    pub regular_imports: Vec<Import>,
    pub namespace_imports: Vec<Import>,
    pub local_exports: Vec<Export>,
    pub indirect_exports: Vec<Export>,
    pub star_exports: Vec<u16>,
}

fn reserve<T>(count: u32) -> Result<Vec<T>> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(count as usize)
        .map_err(|_| resource("ABC data allocation failed"))?;
    Ok(result)
}

fn count(reader: &mut Reader<'_>, width: usize) -> Result<u32> {
    let count = reader.u32()?;
    if count as usize > reader.remaining() / width {
        return Err(invalid("ABC data count exceeds its remaining bytes"));
    }
    Ok(count)
}

fn request_index(reader: &mut Reader<'_>, requests: &[Arc<Name>]) -> Result<u16> {
    let index = reader.u16()?;
    if index as usize >= requests.len() {
        return Err(invalid("ABC module request index is out of range"));
    }
    Ok(index)
}

impl Abc<'_> {
    /// Decode one array without recursively following array or function references.
    pub fn literals(&self, offset: u32) -> Result<Arc<Vec<Literal>>> {
        if let Some(literals) = self.literal_cache.borrow().get(&offset) {
            return Ok(Arc::clone(literals));
        }
        validate_offset(self.data, offset)?;
        let mut reader = Reader::new(self.data, offset, self.data.len())?;
        let values = reader.u32()?;
        if !values.is_multiple_of(2) {
            return Err(invalid("ABC literal tag/value count must be even"));
        }
        let count = values / 2;
        if count as usize > reader.remaining() / 2 {
            return Err(invalid("truncated ABC literal array"));
        }
        self.charge_storage::<Literal>(count as usize)?;
        self.charge_bytes(64)?;
        let mut result = reserve(count)?;
        for _ in 0..count {
            let tag = reader.u8()?;
            let value = match tag {
                0 | 1 | 8 | 0x19 | 0xff => {
                    let value = reader.u8()?;
                    if tag == 1 && value > 1 {
                        return Err(invalid("invalid ABC boolean literal"));
                    }
                    LiteralValue::Byte(value)
                }
                2 | 0x17 => LiteralValue::Integer(reader.u32()?),
                3 => LiteralValue::Float(reader.u32()?),
                4 => LiteralValue::Double(u64::from_le_bytes(reader.take(8)?.try_into().unwrap())),
                5 => LiteralValue::String(self.name(reader.u32()?)?),
                6 | 7 | 0x16 | 0x1a | 0x1b => {
                    let target = reader.u32()?;
                    self.method(target).map_err(|error| {
                        if error.code == "NOT_FOUND" {
                            invalid("ABC literal method is not a declared method")
                        } else {
                            error
                        }
                    })?;
                    LiteralValue::Method(target)
                }
                9 => LiteralValue::Affiliate(reader.u16()?),
                0x18 => {
                    let target = reader.u32()?;
                    validate_offset(self.data, target)?;
                    LiteralValue::Array(target)
                }
                0x0a..=0x15 => {
                    return Err(Error::new(
                        "UNSUPPORTED_INPUT",
                        "static typed literal arrays are outside dynamic ABC support",
                    ));
                }
                _ => return Err(invalid(format!("unknown ABC literal tag {tag:#x}"))),
            };
            result.push(Literal { tag, value });
        }
        let literals = Arc::new(result);
        let mut cache = self.literal_cache.borrow_mut();
        cache
            .try_reserve(1)
            .map_err(|_| resource("ABC literal cache allocation failed"))?;
        cache.insert(offset, Arc::clone(&literals));
        Ok(literals)
    }

    pub fn module(&self, record: &Record) -> Result<Option<Arc<Module>>> {
        let members = self.members(record)?;
        let mut fields = members
            .fields
            .iter()
            .filter(|field| field.name.text == "moduleRecordIdx");
        let Some(field) = fields.next() else {
            return Ok(None);
        };
        if fields.next().is_some() || field.type_id != 6 {
            return Err(invalid("invalid or ambiguous ABC moduleRecordIdx field"));
        }
        let Some(FieldValue::Bits(offset)) = field.value else {
            return Err(invalid("ABC moduleRecordIdx must carry a u32 data offset"));
        };
        if let Some(module) = self.module_cache.borrow().get(&offset) {
            return Ok(Some(Arc::clone(module)));
        }
        self.charge_bytes(std::mem::size_of::<Module>() + 64)?;
        let module = Arc::new(self.module_at(offset)?);
        let mut cache = self.module_cache.borrow_mut();
        cache
            .try_reserve(1)
            .map_err(|_| resource("ABC module cache allocation failed"))?;
        cache.insert(offset, Arc::clone(&module));
        Ok(Some(module))
    }

    fn module_at(&self, offset: u32) -> Result<Module> {
        validate_offset(self.data, offset)?;
        let mut reader = Reader::new(self.data, offset, self.data.len())?;
        let declared_literals = reader.u32()?;
        let size = count(&mut reader, 4)?;
        self.charge_storage::<Arc<Name>>(size as usize)?;
        let mut requests = reserve(size)?;
        for _ in 0..size {
            requests.push(self.name(reader.u32()?)?);
        }
        let size = count(&mut reader, 10)?;
        self.charge_storage::<Import>(size as usize)?;
        let mut regular_imports = reserve(size)?;
        for _ in 0..size {
            let local_name = self.name(reader.u32()?)?;
            let import_name = Some(self.name(reader.u32()?)?);
            let request = request_index(&mut reader, &requests)?;
            regular_imports.push(Import {
                local_name,
                import_name,
                request,
            });
        }
        let size = count(&mut reader, 6)?;
        self.charge_storage::<Import>(size as usize)?;
        let mut namespace_imports = reserve(size)?;
        for _ in 0..size {
            let local_name = self.name(reader.u32()?)?;
            let request = request_index(&mut reader, &requests)?;
            namespace_imports.push(Import {
                local_name,
                import_name: None,
                request,
            });
        }
        let size = count(&mut reader, 8)?;
        self.charge_storage::<Export>(size as usize)?;
        let mut local_exports = reserve(size)?;
        for _ in 0..size {
            let local_name = self.name(reader.u32()?)?;
            let export_name = self.name(reader.u32()?)?;
            local_exports.push(Export {
                local_name,
                export_name,
                request: None,
            });
        }
        let size = count(&mut reader, 10)?;
        self.charge_storage::<Export>(size as usize)?;
        let mut indirect_exports = reserve(size)?;
        for _ in 0..size {
            let export_name = self.name(reader.u32()?)?;
            let local_name = self.name(reader.u32()?)?;
            let request = Some(request_index(&mut reader, &requests)?);
            indirect_exports.push(Export {
                local_name,
                export_name,
                request,
            });
        }
        let size = count(&mut reader, 2)?;
        self.charge_storage::<u16>(size as usize)?;
        let mut star_exports = reserve(size)?;
        for _ in 0..size {
            star_exports.push(request_index(&mut reader, &requests)?);
        }
        Ok(Module {
            offset,
            declared_literals,
            requests,
            regular_imports,
            namespace_imports,
            local_exports,
            indirect_exports,
            star_exports,
        })
    }
}
