//! JVM metadata and Code attributes from one immutable class-file buffer.
use std::ops::Range;

use crate::types::{MethodDescriptor, Type, Utf16String, field_type as parse_field_type};
use anyhow::{Context, Result, bail, ensure};

#[derive(Debug, Clone)]
pub(crate) enum Constant {
    Empty,
    Utf8(Vec<u16>),
    Integer(i32),
    Float(u32),
    Long(i64),
    Double(u64),
    Class(u16),
    String(u16),
    Member {
        tag: u8,
        owner: u16,
        name_type: u16,
    },
    NameType(u16, u16),
    Handle(u8, u16),
    MethodType(u16),
    Dynamic {
        tag: u8,
        bootstrap: u16,
        name_type: u16,
    },
    Module(u16),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Field {
    pub name: String,
    pub descriptor: String,
    pub access: u16,
    pub signature: Option<String>,
    pub constant_index: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct Method {
    pub name: String,
    pub descriptor: String,
    pub access: u16,
    pub signature: Option<String>,
    pub exceptions: Vec<String>,
    pub parameter_names: Vec<Option<String>>,
    pub code: Option<Code>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Handler {
    pub start: u16,
    pub end: u16,
    pub target: u16,
    pub catch_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Local {
    pub start: u16,
    pub length: u16,
    pub name: String,
    pub descriptor: String,
    pub slot: u16,
}

#[derive(Debug, Clone)]
pub struct Code {
    pub max_stack: u16,
    pub max_locals: u16,
    pub range: Range<usize>,
    pub handlers: Vec<Handler>,
    pub lines: Vec<(u16, u16)>,
    pub locals: Vec<Local>,
}

#[derive(Debug)]
pub struct ClassFile {
    pub name: String,
    pub super_name: Option<String>,
    pub interfaces: Vec<String>,
    pub access: u16,
    pub major: u16,
    pub minor: u16,
    pub signature: Option<String>,
    pub source_file: Option<String>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub bytes: Vec<u8>,
    pub(crate) pool: Vec<Constant>,
    pub(crate) metadata_bytes: usize,
}

impl ClassFile {
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        Self::parse_bounded(bytes, 16 * 1024 * 1024)
    }

    pub(crate) fn parse_bounded(bytes: Vec<u8>, metadata_limit: usize) -> Result<Self> {
        let mut r = Reader::new(&bytes);
        ensure!(
            r.u4()? == 0xcafe_babe,
            "INVALID_INPUT: expected JVM class-file magic"
        );
        let minor = r.u2()?;
        let major = r.u2()?;
        ensure!(major >= 45, "INVALID_INPUT: class-file version {major}");
        let count = usize::from(r.u2()?);
        ensure!(count > 0, "INVALID_INPUT: empty constant pool");
        let mut pool = vec![Constant::Empty; count];
        let mut i = 1;
        while i < count {
            let tag = r.u1()?;
            pool[i] = match tag {
                1 => {
                    let n = usize::from(r.u2()?);
                    Constant::Utf8(modified_utf8(r.take(n)?)?)
                }
                3 => Constant::Integer(r.u4()? as i32),
                4 => Constant::Float(r.u4()?),
                5 => Constant::Long(r.u8()? as i64),
                6 => Constant::Double(r.u8()?),
                7 => Constant::Class(r.u2()?),
                8 => Constant::String(r.u2()?),
                9..=11 => Constant::Member {
                    tag,
                    owner: r.u2()?,
                    name_type: r.u2()?,
                },
                12 => Constant::NameType(r.u2()?, r.u2()?),
                15 => Constant::Handle(r.u1()?, r.u2()?),
                16 => Constant::MethodType(r.u2()?),
                17 | 18 => Constant::Dynamic {
                    tag,
                    bootstrap: r.u2()?,
                    name_type: r.u2()?,
                },
                19 | 20 => Constant::Module(r.u2()?),
                _ => bail!("INVALID_INPUT: constant-pool tag {tag}"),
            };
            if matches!(tag, 5 | 6) {
                i += 1;
                ensure!(i < count, "INVALID_INPUT: missing wide constant slot");
            }
            i += 1;
        }
        let mut metadata_bytes = pool.len() * std::mem::size_of::<Constant>();
        metadata_bytes += pool
            .iter()
            .map(|c| match c {
                Constant::Utf8(v) => v.capacity() * 2,
                _ => 0,
            })
            .sum::<usize>();
        metadata_charge(&mut metadata_bytes, 0, metadata_limit)?;
        let access = r.u2()?;
        let name = class_name(&pool, r.u2()?)?;
        ensure!(
            !name.starts_with('['),
            "INVALID_INPUT: array class declaration"
        );
        let super_index = r.u2()?;
        let super_name = (super_index != 0)
            .then(|| class_name(&pool, super_index))
            .transpose()?;
        let mut interfaces = Vec::new();
        for _ in 0..r.u2()? {
            let interface = class_name(&pool, r.u2()?)?;
            metadata_charge(
                &mut metadata_bytes,
                interface.len() + 2 * std::mem::size_of::<String>(),
                metadata_limit,
            )?;
            interfaces.push(interface);
        }
        let mut fields = Vec::new();
        for _ in 0..r.u2()? {
            let access = r.u2()?;
            let name = utf8(&pool, r.u2()?)?;
            let descriptor = utf8(&pool, r.u2()?)?;
            field_type(&descriptor)?;
            let mut field = Field {
                name,
                descriptor,
                access,
                signature: None,
                constant_index: None,
            };
            for attribute in attributes(&mut r, &pool)? {
                let mut a = attribute.reader();
                match attribute.name.as_str() {
                    "Signature" => {
                        field.signature = Some(utf8(&pool, a.u2()?)?);
                        a.finish()?;
                    }
                    "ConstantValue" => {
                        let index = a.u2()?;
                        constant(&pool, index)?;
                        field.constant_index = Some(index);
                        a.finish()?;
                    }
                    _ => {}
                }
            }
            metadata_charge(
                &mut metadata_bytes,
                2 * std::mem::size_of::<Field>()
                    + field.name.capacity()
                    + field.descriptor.capacity()
                    + field.signature.as_ref().map_or(0, String::capacity),
                metadata_limit,
            )?;
            fields.push(field);
        }
        let mut methods = Vec::new();
        for _ in 0..r.u2()? {
            let access = r.u2()?;
            let name = utf8(&pool, r.u2()?)?;
            let descriptor = utf8(&pool, r.u2()?)?;
            method_type(&descriptor)?;
            let mut method = Method {
                name,
                descriptor,
                access,
                signature: None,
                exceptions: Vec::new(),
                parameter_names: Vec::new(),
                code: None,
            };
            for attribute in attributes(&mut r, &pool)? {
                let mut a = attribute.reader();
                match attribute.name.as_str() {
                    "Code" => {
                        ensure!(
                            method.code.is_none(),
                            "INVALID_INPUT: duplicate Code attribute"
                        );
                        method.code = Some(read_code(&mut a, &pool)?);
                    }
                    "Signature" => {
                        method.signature = Some(utf8(&pool, a.u2()?)?);
                        a.finish()?;
                    }
                    "Exceptions" => {
                        for _ in 0..a.u2()? {
                            method.exceptions.push(class_name(&pool, a.u2()?)?);
                        }
                        a.finish()?;
                    }
                    "MethodParameters" => {
                        for _ in 0..a.u1()? {
                            let n = a.u2()?;
                            method
                                .parameter_names
                                .push((n != 0).then(|| utf8(&pool, n)).transpose()?);
                            a.u2()?;
                        }
                        a.finish()?;
                    }
                    _ => {}
                }
            }
            ensure!(
                (access & 0x0500 != 0) == method.code.is_none(),
                "INVALID_INPUT: inconsistent Code attribute for {}",
                method.name
            );
            metadata_charge(
                &mut metadata_bytes,
                method_metadata_bytes(&method),
                metadata_limit,
            )?;
            methods.push(method);
        }
        let mut signature = None;
        let mut source_file = None;
        for attribute in attributes(&mut r, &pool)? {
            let mut a = attribute.reader();
            match attribute.name.as_str() {
                "Signature" => {
                    signature = Some(utf8(&pool, a.u2()?)?);
                    a.finish()?;
                }
                "SourceFile" => {
                    source_file = Some(utf8(&pool, a.u2()?)?);
                    a.finish()?;
                }
                _ => {}
            }
        }
        r.finish()?;
        metadata_charge(
            &mut metadata_bytes,
            name.capacity()
                + super_name.as_ref().map_or(0, String::capacity)
                + signature.as_ref().map_or(0, String::capacity)
                + source_file.as_ref().map_or(0, String::capacity),
            metadata_limit,
        )?;
        Ok(Self {
            name,
            super_name,
            interfaces,
            access,
            major,
            minor,
            signature,
            source_file,
            fields,
            methods,
            bytes,
            pool,
            metadata_bytes,
        })
    }

    pub fn bytecode(&self, method: &Method) -> Option<&[u8]> {
        method
            .code
            .as_ref()
            .map(|code| &self.bytes[code.range.clone()])
    }

    pub(crate) fn member(&self, index: u16) -> Result<(u8, String, String, String)> {
        let Constant::Member {
            tag,
            owner,
            name_type,
        } = constant(&self.pool, index)?
        else {
            bail!("INVALID_INPUT: constant {index} is not a member reference");
        };
        let Constant::NameType(name, descriptor) = constant(&self.pool, *name_type)? else {
            bail!("INVALID_INPUT: invalid member name/type");
        };
        Ok((
            *tag,
            class_name(&self.pool, *owner)?,
            utf8(&self.pool, *name)?,
            utf8(&self.pool, *descriptor)?,
        ))
    }

    pub(crate) fn class_type(&self, index: u16) -> Result<Type> {
        type_for_class(&class_name(&self.pool, index)?)
    }

    pub(crate) fn string(&self, index: u16) -> Result<Utf16String> {
        let Constant::String(value) = constant(&self.pool, index)? else {
            bail!("INVALID_INPUT: expected string constant");
        };
        let Constant::Utf8(units) = constant(&self.pool, *value)? else {
            bail!("INVALID_INPUT: invalid string constant");
        };
        Ok(Utf16String::from_utf16(units.clone()))
    }

    /// Enumerate every `CONSTANT_String` literal in constant-pool order.
    /// Returns the pool index of the `String` entry and its decoded UTF-16
    /// value. Only string literals are reported, never class names, member
    /// names, descriptors or other Utf8 pool entries.
    pub fn string_constants(&self) -> Vec<(u16, Utf16String)> {
        let mut out = Vec::new();
        for (position, constant) in self.pool.iter().enumerate() {
            let Constant::String(value) = constant else {
                continue;
            };
            if let Some(Constant::Utf8(units)) = self.pool.get(*value as usize) {
                out.push((position as u16, Utf16String::from_utf16(units.clone())));
            }
        }
        out
    }

    /// Resolve a constant-pool entry a code instruction references into a typed
    /// cross-reference target, or `None` when the entry is not a type/member
    /// reference. Used by `findrefs` to classify instruction operands.
    pub fn reference_at(&self, index: u16) -> Option<Reference> {
        match constant(&self.pool, index).ok()? {
            Constant::Class(_) => Some(Reference::Type {
                name: self.class_type(index).ok()?.descriptor().into_owned(),
            }),
            Constant::Member { tag, .. } => {
                let (_, owner, name, descriptor) = self.member(index).ok()?;
                let kind = if *tag == 9 {
                    MemberKind::Field
                } else {
                    MemberKind::Method
                };
                Some(Reference::Member {
                    kind,
                    owner,
                    name,
                    descriptor,
                })
            }
            _ => None,
        }
    }
}

/// A field vs method/interface-method constant-pool member reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Field,
    Method,
}

/// A classified cross-reference target resolved from a code instruction.
#[derive(Debug, Clone)]
pub enum Reference {
    /// A `CONSTANT_Class` operand (`new`, `checkcast`, `instanceof`, arrays).
    Type { name: String },
    /// A field or method member reference (`getfield`, `invokevirtual`, ...).
    Member {
        kind: MemberKind,
        owner: String,
        name: String,
        descriptor: String,
    },
}

pub(crate) fn method_type(descriptor: &str) -> Result<MethodDescriptor> {
    MethodDescriptor::parse(descriptor)
        .with_context(|| format!("INVALID_INPUT: method descriptor {descriptor}"))
}

pub(crate) fn field_type(descriptor: &str) -> Result<Type> {
    parse_field_type(descriptor)
        .with_context(|| format!("INVALID_INPUT: field descriptor {descriptor}"))
}

pub(crate) fn type_for_class(name: &str) -> Result<Type> {
    crate::types::type_for_class(name)
}

pub(crate) fn constant(pool: &[Constant], index: u16) -> Result<&Constant> {
    pool.get(usize::from(index))
        .filter(|c| !matches!(c, Constant::Empty))
        .with_context(|| format!("INVALID_INPUT: invalid constant-pool index {index}"))
}

fn utf8(pool: &[Constant], index: u16) -> Result<String> {
    let Constant::Utf8(units) = constant(pool, index)? else {
        bail!("INVALID_INPUT: expected UTF8 constant at {index}");
    };
    ensure!(
        units.len() <= 4096,
        "RESOURCE_LIMIT: metadata string length exceeded"
    );
    String::from_utf16(units).context("INVALID_INPUT: non-Unicode metadata name")
}

fn class_name(pool: &[Constant], index: u16) -> Result<String> {
    let Constant::Class(name) = constant(pool, index)? else {
        bail!("INVALID_INPUT: expected class constant at {index}");
    };
    let name = utf8(pool, *name)?;
    ensure!(!name.is_empty(), "INVALID_INPUT: empty class name");
    type_for_class(&name)?;
    Ok(name)
}

fn read_code(r: &mut Reader<'_>, pool: &[Constant]) -> Result<Code> {
    let max_stack = r.u2()?;
    let max_locals = r.u2()?;
    let length = r.u4()? as usize;
    ensure!(
        length > 0 && length < 65536,
        "INVALID_INPUT: Code length {length}"
    );
    let start = r.base + r.pos;
    r.take(length)?;
    let mut code = Code {
        max_stack,
        max_locals,
        range: start..start + length,
        handlers: Vec::new(),
        lines: Vec::new(),
        locals: Vec::new(),
    };
    for _ in 0..r.u2()? {
        let start = r.u2()?;
        let end = r.u2()?;
        let target = r.u2()?;
        let catch = r.u2()?;
        ensure!(
            start < end && usize::from(end) <= length && usize::from(target) < length,
            "INVALID_INPUT: exception table range"
        );
        code.handlers.push(Handler {
            start,
            end,
            target,
            catch_type: (catch != 0).then(|| class_name(pool, catch)).transpose()?,
        });
    }
    for attribute in attributes(r, pool)? {
        let mut a = attribute.reader();
        match attribute.name.as_str() {
            "LineNumberTable" => {
                for _ in 0..a.u2()? {
                    let offset = a.u2()?;
                    ensure!(
                        usize::from(offset) < length,
                        "INVALID_INPUT: line number offset"
                    );
                    code.lines.push((offset, a.u2()?));
                }
                a.finish()?;
            }
            "LocalVariableTable" => {
                for _ in 0..a.u2()? {
                    let start = a.u2()?;
                    let span = a.u2()?;
                    let name = utf8(pool, a.u2()?)?;
                    let descriptor = utf8(pool, a.u2()?)?;
                    let slot = a.u2()?;
                    let ty = field_type(&descriptor)?;
                    ensure!(
                        usize::from(start) + usize::from(span) <= length
                            && u32::from(slot) + if ty.is_wide() { 2 } else { 1 }
                                <= u32::from(max_locals),
                        "INVALID_INPUT: local variable range"
                    );
                    code.locals.push(Local {
                        start,
                        length: span,
                        name,
                        descriptor,
                        slot,
                    });
                }
                a.finish()?;
            }
            _ => {}
        }
    }
    r.finish()?;
    Ok(code)
}

struct Attribute<'a> {
    name: String,
    bytes: &'a [u8],
    base: usize,
}
impl<'a> Attribute<'a> {
    fn reader(&self) -> Reader<'a> {
        Reader {
            bytes: self.bytes,
            pos: 0,
            base: self.base,
        }
    }
}
fn attributes<'a>(r: &mut Reader<'a>, pool: &[Constant]) -> Result<Vec<Attribute<'a>>> {
    let count = r.u2()?;
    let mut result = Vec::new();
    let mut metadata_bytes = 0;
    for _ in 0..count {
        let name = utf8(pool, r.u2()?)?;
        let length = r.u4()? as usize;
        let base = r.base + r.pos;
        let bytes = r.take(length)?;
        metadata_charge(
            &mut metadata_bytes,
            name.capacity() + 2 * std::mem::size_of::<Attribute<'_>>(),
            16 * 1024 * 1024,
        )?;
        result.push(Attribute { name, bytes, base });
    }
    Ok(result)
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pub(crate) pos: usize,
    base: usize,
}
impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            base: 0,
        }
    }
    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(length)
            .context("INVALID_INPUT: range overflow")?;
        let result = self
            .bytes
            .get(self.pos..end)
            .context("INVALID_INPUT: truncated class file")?;
        self.pos = end;
        Ok(result)
    }
    pub(crate) fn u1(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u2(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into()?))
    }
    pub(crate) fn u4(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }
    fn u8(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into()?))
    }
    fn finish(&self) -> Result<()> {
        ensure!(
            self.pos == self.bytes.len(),
            "INVALID_INPUT: trailing attribute/class bytes"
        );
        Ok(())
    }
}

fn modified_utf8(bytes: &[u8]) -> Result<Vec<u16>> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let a = bytes[i];
        i += 1;
        let continuation = |index: usize| -> Result<u16> {
            let b = *bytes
                .get(index)
                .context("INVALID_INPUT: truncated modified UTF8")?;
            ensure!(
                b & 0xc0 == 0x80,
                "INVALID_INPUT: modified UTF8 continuation"
            );
            Ok(u16::from(b & 0x3f))
        };
        let unit = match a {
            1..=0x7f => u16::from(a),
            0xc0..=0xdf => {
                let v = (u16::from(a & 0x1f) << 6) | continuation(i)?;
                i += 1;
                ensure!(
                    v >= 0x80 || (a == 0xc0 && v == 0),
                    "INVALID_INPUT: overlong modified UTF8"
                );
                v
            }
            0xe0..=0xef => {
                let v =
                    (u16::from(a & 0x0f) << 12) | (continuation(i)? << 6) | continuation(i + 1)?;
                i += 2;
                ensure!(v >= 0x800, "INVALID_INPUT: overlong modified UTF8");
                v
            }
            _ => bail!("INVALID_INPUT: invalid modified UTF8 byte"),
        };
        result.push(unit);
    }
    Ok(result)
}

fn metadata_charge(used: &mut usize, amount: usize, limit: usize) -> Result<()> {
    *used = used
        .checked_add(amount)
        .context("RESOURCE_LIMIT: metadata allocation overflow")?;
    ensure!(
        *used <= limit,
        "RESOURCE_LIMIT: decoded class metadata budget exceeded"
    );
    Ok(())
}

fn method_metadata_bytes(method: &Method) -> usize {
    let mut size = 2 * std::mem::size_of::<Method>()
        + method.name.capacity()
        + method.descriptor.capacity()
        + method.signature.as_ref().map_or(0, String::capacity);
    size += method.exceptions.capacity() * std::mem::size_of::<String>()
        + method
            .exceptions
            .iter()
            .map(String::capacity)
            .sum::<usize>();
    size += method.parameter_names.capacity() * std::mem::size_of::<Option<String>>()
        + method
            .parameter_names
            .iter()
            .flatten()
            .map(String::capacity)
            .sum::<usize>();
    if let Some(code) = &method.code {
        size += code.handlers.capacity() * std::mem::size_of::<Handler>()
            + code
                .handlers
                .iter()
                .filter_map(|h| h.catch_type.as_ref())
                .map(String::capacity)
                .sum::<usize>();
        size += code.lines.capacity() * std::mem::size_of::<(u16, u16)>();
        size += code.locals.capacity() * std::mem::size_of::<Local>()
            + code
                .locals
                .iter()
                .map(|l| l.name.capacity() + l.descriptor.capacity())
                .sum::<usize>();
    }
    size
}
