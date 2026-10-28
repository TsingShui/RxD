//! Structural signature identity. Display strings are never used for matching.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use crate::{Input, input::Work};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeSignature {
    Primitive {
        element: u8,
    },
    Named {
        element: u8,
        token: u32,
    },
    Specification {
        element: u8,
        value: Box<Self>,
    },
    TypeParameter {
        index: u32,
    },
    MethodParameter {
        index: u32,
    },
    Pointer {
        value: Box<Self>,
    },
    ByRef {
        value: Box<Self>,
    },
    Vector {
        value: Box<Self>,
    },
    Pinned {
        value: Box<Self>,
    },
    Array {
        value: Box<Self>,
        rank: u32,
        sizes: Vec<u32>,
        bounds: Vec<i32>,
    },
    Constructed {
        element: u8,
        token: u32,
        arguments: Vec<Self>,
        constructor_arguments: Vec<Self>,
    },
    Modified {
        required: bool,
        token: u32,
        arguments: Vec<Self>,
        value: Box<Self>,
    },
    Function {
        signature: Box<MethodSignature>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MethodSignature {
    pub flags: u8,
    pub generic_arity: u32,
    pub sentinel: Option<usize>,
    pub return_type: TypeSignature,
    pub parameters: Vec<TypeSignature>,
}

impl MethodSignature {
    pub(crate) fn charge_clone(&self, work: &mut Work, depth: usize) -> Result<()> {
        work.retain(std::mem::size_of::<Self>())?;
        self.return_type.charge_clone(work, depth + 1)?;
        charge_types(&self.parameters, work, depth + 1)
    }
}

pub(crate) fn charge_types(types: &[TypeSignature], work: &mut Work, depth: usize) -> Result<()> {
    for ty in types {
        ty.charge_clone(work, depth)?;
    }
    Ok(())
}

impl TypeSignature {
    // Generic substitution copies the whole inserted AST once. Charge its
    // expanded storage before clone(), even when a small blob repeats !0.
    pub(crate) fn charge_clone(&self, work: &mut Work, depth: usize) -> Result<()> {
        ensure!(depth <= 64, "RESOURCE_LIMIT: semantic signature nesting");
        work.retain(std::mem::size_of::<Self>())?;
        match self {
            Self::Specification { value, .. }
            | Self::Pointer { value }
            | Self::ByRef { value }
            | Self::Vector { value }
            | Self::Pinned { value } => value.charge_clone(work, depth + 1),
            Self::Array {
                value,
                sizes,
                bounds,
                ..
            } => {
                work.retain(
                    sizes.len() * std::mem::size_of::<u32>()
                        + bounds.len() * std::mem::size_of::<i32>(),
                )?;
                value.charge_clone(work, depth + 1)
            }
            Self::Constructed {
                arguments,
                constructor_arguments,
                ..
            } => {
                charge_types(arguments, work, depth + 1)?;
                charge_types(constructor_arguments, work, depth + 1)
            }
            Self::Modified {
                arguments, value, ..
            } => {
                charge_types(arguments, work, depth + 1)?;
                value.charge_clone(work, depth + 1)
            }
            Self::Function { signature } => signature.charge_clone(work, depth + 1),
            _ => Ok(()),
        }
    }
}

pub(crate) fn method(input: &Input<'_>, bytes: &[u8], work: &mut Work) -> Result<MethodSignature> {
    let mut parser = Parser::new(input, bytes, work);
    let result = parser.method(0)?;
    parser.end()?;
    Ok(result)
}

pub(crate) fn field(input: &Input<'_>, bytes: &[u8], work: &mut Work) -> Result<TypeSignature> {
    let mut parser = Parser::new(input, bytes, work);
    ensure!(parser.byte()? == 6, "INVALID_INPUT: field signature tag");
    let result = parser.ty(0, false)?;
    parser.end()?;
    Ok(result)
}

pub(crate) fn ty(input: &Input<'_>, bytes: &[u8], work: &mut Work) -> Result<TypeSignature> {
    let mut parser = Parser::new(input, bytes, work);
    let result = parser.ty(0, false)?;
    parser.end()?;
    Ok(result)
}

pub(crate) fn arguments(
    input: &Input<'_>,
    bytes: &[u8],
    work: &mut Work,
) -> Result<Vec<TypeSignature>> {
    let mut parser = Parser::new(input, bytes, work);
    ensure!(
        parser.byte()? == 0x0a,
        "INVALID_INPUT: MethodSpec signature tag"
    );
    let count = parser.count()?;
    let mut arguments = Vec::new();
    for _ in 0..count {
        arguments.push(parser.ty(0, false)?);
    }
    parser.end()?;
    Ok(arguments)
}

pub(crate) fn locals(
    input: &Input<'_>,
    bytes: &[u8],
    work: &mut Work,
) -> Result<Vec<TypeSignature>> {
    let mut parser = Parser::new(input, bytes, work);
    ensure!(parser.byte()? == 7, "INVALID_INPUT: locals signature tag");
    let count = parser.count()?;
    let result = (0..count)
        .map(|_| parser.ty(0, false))
        .collect::<Result<_>>()?;
    parser.end()?;
    Ok(result)
}

pub(crate) fn property(
    input: &Input<'_>,
    bytes: &[u8],
    work: &mut Work,
) -> Result<MethodSignature> {
    let mut parser = Parser::new(input, bytes, work);
    let flags = parser.byte()?;
    ensure!(
        flags & !0x20 == 8,
        "INVALID_INPUT: property calling convention"
    );
    let count = parser.count()?;
    let return_type = parser.ty(0, false)?;
    let parameters = (0..count)
        .map(|_| parser.ty(0, false))
        .collect::<Result<_>>()?;
    parser.end()?;
    Ok(MethodSignature {
        flags,
        generic_arity: 0,
        sentinel: None,
        return_type,
        parameters,
    })
}

impl MethodSignature {
    pub(crate) fn display(&self, work: &mut Work) -> Result<String> {
        let parameters = display_types(&self.parameters, work)?;
        let return_type = self.return_type.display(work)?;
        let text = format!(
            "flags=0x{:02x};generic={};sentinel={:?};{}({})",
            self.flags,
            self.generic_arity,
            self.sentinel,
            return_type,
            parameters.join(",")
        );
        work.charge(text.len())?;
        Ok(text)
    }
}

pub(crate) fn display_types(types: &[TypeSignature], work: &mut Work) -> Result<Vec<String>> {
    types.iter().map(|ty| ty.display(work)).collect()
}

impl TypeSignature {
    pub(crate) fn display(&self, work: &mut Work) -> Result<String> {
        let named = |element, token| {
            format!(
                "{}(0x{token:08x})",
                if element == 0x11 {
                    "valuetype"
                } else {
                    "class"
                }
            )
        };
        let text = match self {
            Self::Primitive { element } => match element {
                1 => "void",
                2 => "bool",
                3 => "char",
                4 => "int8",
                5 => "uint8",
                6 => "int16",
                7 => "uint16",
                8 => "int32",
                9 => "uint32",
                10 => "int64",
                11 => "uint64",
                12 => "float32",
                13 => "float64",
                14 => "string",
                0x16 => "typedref",
                0x18 => "native int",
                0x19 => "native uint",
                0x1c => "object",
                _ => unreachable!("parsed primitive"),
            }
            .to_owned(),
            Self::Named { element, token } => named(*element, *token),
            Self::Specification { element, value } => format!(
                "{}({})",
                if *element == 0x11 {
                    "valuetype"
                } else {
                    "class"
                },
                value.display(work)?
            ),
            Self::TypeParameter { index } => format!("!{index}"),
            Self::MethodParameter { index } => format!("!!{index}"),
            Self::Pointer { value } => format!("{}*", value.display(work)?),
            Self::ByRef { value } => format!("{}&", value.display(work)?),
            Self::Vector { value } => format!("{}[]", value.display(work)?),
            Self::Pinned { value } => format!("pinned {}", value.display(work)?),
            Self::Array {
                value,
                rank,
                sizes,
                bounds,
            } => format!(
                "array({};rank={rank};sizes={sizes:?};bounds={bounds:?})",
                value.display(work)?
            ),
            Self::Constructed {
                element,
                token,
                arguments,
                ..
            } => format!(
                "genericinst({}<{}>)",
                named(*element, *token),
                display_types(arguments, work)?.join(",")
            ),
            Self::Modified {
                required,
                token,
                value,
                ..
            } => format!(
                "{}(0x{token:08x}) {}",
                if *required { "modreq" } else { "modopt" },
                value.display(work)?
            ),
            Self::Function { signature } => format!("fnptr({})", signature.display(work)?),
        };
        work.charge(text.len())?;
        Ok(text)
    }
}

struct Parser<'a, 'w> {
    input: &'a Input<'a>,
    bytes: &'a [u8],
    pos: usize,
    work: &'w mut Work,
}
impl<'a, 'w> Parser<'a, 'w> {
    fn end(&self) -> Result<()> {
        ensure!(
            self.pos == self.bytes.len(),
            "INVALID_INPUT: trailing signature bytes"
        );
        Ok(())
    }
    fn new(input: &'a Input<'a>, bytes: &'a [u8], work: &'w mut Work) -> Self {
        Self {
            input,
            bytes,
            pos: 0,
            work,
        }
    }
    fn byte(&mut self) -> Result<u8> {
        self.work.charge(1)?;
        let byte = *self
            .bytes
            .get(self.pos)
            .context("INVALID_INPUT: truncated signature")?;
        self.pos += 1;
        Ok(byte)
    }
    fn uint(&mut self) -> Result<u32> {
        crate::metadata::compressed(self.bytes, &mut self.pos, self.work).map(|(v, _)| v)
    }
    fn count(&mut self) -> Result<usize> {
        let count = self.uint()? as usize;
        ensure!(
            count <= self.bytes.len(),
            "INVALID_INPUT: signature collection count"
        );
        self.work.retain(
            count
                .checked_mul(std::mem::size_of::<TypeSignature>())
                .context("RESOURCE_LIMIT: signature collection")?,
        )?;
        Ok(count)
    }
    fn token(&mut self) -> Result<u32> {
        let coded = self.uint()?;
        let table = match coded & 3 {
            0 => 2,
            1 => 1,
            2 => 27,
            _ => bail!("INVALID_INPUT: signature type token tag"),
        };
        let token = (table << 24) | (coded >> 2);
        self.input.check_token(token, &[1, 2, 27])?;
        Ok(token)
    }
    fn method(&mut self, depth: usize) -> Result<MethodSignature> {
        ensure!(depth <= 64, "RESOURCE_LIMIT: signature nesting");
        let flags = self.byte()?;
        ensure!(
            flags & 0x80 == 0
                && matches!(flags & 15, 0..=5 | 9 | 11)
                && (flags & 0x40 == 0 || flags & 0x20 != 0),
            "INVALID_INPUT: calling convention flags"
        );
        let generic_arity = if flags & 0x10 != 0 { self.uint()? } else { 0 };
        let count = self.count()?;
        let return_type = self.ty(depth + 1, true)?;
        let mut parameters = Vec::new();
        let mut sentinel = None;
        for _ in 0..count {
            if self.bytes.get(self.pos) == Some(&0x41) {
                ensure!(
                    matches!(flags & 15, 5 | 11) && sentinel.is_none(),
                    "INVALID_INPUT: signature sentinel"
                );
                self.byte()?;
                sentinel = Some(parameters.len());
            }
            parameters.push(self.ty(depth + 1, false)?);
        }
        Ok(MethodSignature {
            flags,
            generic_arity,
            sentinel,
            return_type,
            parameters,
        })
    }
    fn ty(&mut self, depth: usize, allow_void: bool) -> Result<TypeSignature> {
        ensure!(depth <= 64, "RESOURCE_LIMIT: signature nesting");
        self.work.retain(std::mem::size_of::<TypeSignature>())?;
        let element = self.byte()?;
        Ok(match element {
            1 if allow_void => TypeSignature::Primitive { element },
            2..=14 | 0x16 | 0x18 | 0x19 | 0x1c => TypeSignature::Primitive { element },
            0x11 | 0x12 => TypeSignature::Named {
                element,
                token: self.token()?,
            },
            0x13 => TypeSignature::TypeParameter {
                index: self.uint()?,
            },
            0x1e => TypeSignature::MethodParameter {
                index: self.uint()?,
            },
            0x0f => TypeSignature::Pointer {
                value: Box::new(self.ty(depth + 1, true)?),
            },
            0x10 => TypeSignature::ByRef {
                value: Box::new(self.ty(depth + 1, false)?),
            },
            0x1d => TypeSignature::Vector {
                value: Box::new(self.ty(depth + 1, false)?),
            },
            0x45 => TypeSignature::Pinned {
                value: Box::new(self.ty(depth + 1, false)?),
            },
            0x1f | 0x20 => TypeSignature::Modified {
                required: element == 0x1f,
                token: self.token()?,
                arguments: Vec::new(),
                value: Box::new(self.ty(depth + 1, allow_void)?),
            },
            0x15 => {
                let element = self.byte()?;
                ensure!(
                    matches!(element, 0x11 | 0x12),
                    "INVALID_INPUT: generic type kind"
                );
                let token = self.token()?;
                let count = self.count()?;
                let mut arguments = Vec::new();
                for _ in 0..count {
                    arguments.push(self.ty(depth + 1, false)?);
                }
                TypeSignature::Constructed {
                    element,
                    token,
                    arguments,
                    constructor_arguments: Vec::new(),
                }
            }
            0x14 => {
                let value = Box::new(self.ty(depth + 1, false)?);
                let rank = self.uint()?;
                let ns = self.count()?;
                ensure!(
                    rank > 0 && ns <= rank as usize,
                    "INVALID_INPUT: array shape"
                );
                let mut sizes = Vec::new();
                for _ in 0..ns {
                    sizes.push(self.uint()?);
                }
                let nb = self.count()?;
                ensure!(nb <= rank as usize, "INVALID_INPUT: array lower bounds");
                let mut bounds = Vec::new();
                for _ in 0..nb {
                    let (value, width) =
                        crate::metadata::compressed(self.bytes, &mut self.pos, self.work)?;
                    let mut bound = (value >> 1) as i32;
                    if value & 1 != 0 {
                        bound |= match width {
                            1 => !0x3f,
                            2 => !0x1fff,
                            _ => !0x0fff_ffff,
                        };
                    }
                    bounds.push(bound);
                }
                TypeSignature::Array {
                    value,
                    rank,
                    sizes,
                    bounds,
                }
            }
            0x1b => TypeSignature::Function {
                signature: Box::new(self.method(depth + 1)?),
            },
            _ => bail!("INVALID_INPUT: unsupported signature element 0x{element:02x}"),
        })
    }
}
