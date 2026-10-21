//! JVM descriptor types and decoded constant-pool strings.
//!
//! These are the front end's own types: the operand stack and the local slots
//! hold [`Type`], and the descriptor it renders is the same string the IR carries,
//! so a type travels into the backend without translation.

use std::borrow::Cow;

use anyhow::{Context, Result, bail, ensure};

/// A descriptor must fit its CONSTANT_Utf8 u16 byte-length field (JVMS 4.4.7).
const MAX_DESCRIPTOR: usize = u16::MAX as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
    Boolean,
    Byte,
    Short,
    Char,
    Int,
    Float,
    Long,
    Double,
}

impl Primitive {
    fn from_char(value: char) -> Option<Self> {
        Some(match value {
            'Z' => Self::Boolean,
            'B' => Self::Byte,
            'S' => Self::Short,
            'C' => Self::Char,
            'I' => Self::Int,
            'F' => Self::Float,
            'J' => Self::Long,
            'D' => Self::Double,
            _ => return None,
        })
    }

    pub fn descriptor(self) -> &'static str {
        match self {
            Self::Boolean => "Z",
            Self::Byte => "B",
            Self::Short => "S",
            Self::Char => "C",
            Self::Int => "I",
            Self::Float => "F",
            Self::Long => "J",
            Self::Double => "D",
        }
    }

    fn is_wide(self) -> bool {
        matches!(self, Self::Long | Self::Double)
    }

    pub fn java_name(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Byte => "byte",
            Self::Short => "short",
            Self::Char => "char",
            Self::Int => "int",
            Self::Float => "float",
            Self::Long => "long",
            Self::Double => "double",
        }
    }

    fn is_integral(self) -> bool {
        matches!(
            self,
            Self::Boolean | Self::Byte | Self::Short | Self::Char | Self::Int
        )
    }
}

/// A value's verification type.
///
/// `Unknown` carries the primitives the value could still be: empty means "a
/// reference of unknown class" (the join of incomparable references), non-empty
/// means the join of those primitives, e.g. `Unknown([Boolean, Byte])` right after
/// `baload`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Void,
    Primitive(Primitive),
    /// Internal name, e.g. `java/lang/String`.
    Object(String),
    Array(Box<Type>),
    Unknown(Vec<Primitive>),
}

impl Type {
    pub const BOOLEAN: Self = Self::Primitive(Primitive::Boolean);
    pub const BYTE: Self = Self::Primitive(Primitive::Byte);
    pub const SHORT: Self = Self::Primitive(Primitive::Short);
    pub const CHAR: Self = Self::Primitive(Primitive::Char);
    pub const INT: Self = Self::Primitive(Primitive::Int);
    pub const FLOAT: Self = Self::Primitive(Primitive::Float);
    pub const LONG: Self = Self::Primitive(Primitive::Long);
    pub const DOUBLE: Self = Self::Primitive(Primitive::Double);

    pub fn object(name: &str) -> Self {
        Self::Object(name.to_owned())
    }

    pub fn string() -> Self {
        Self::object("java/lang/String")
    }

    /// A reference whose class the bytecode does not pin down.
    pub fn unknown_object() -> Self {
        Self::Unknown(Vec::new())
    }

    pub fn array(element: Type) -> Self {
        Self::Array(Box::new(element))
    }

    pub fn is_void(&self) -> bool {
        matches!(self, Self::Void)
    }

    pub fn is_wide(&self) -> bool {
        match self {
            Self::Primitive(primitive) => primitive.is_wide(),
            Self::Unknown(choices) => choices.iter().any(|primitive| primitive.is_wide()),
            _ => false,
        }
    }

    pub fn is_reference(&self) -> bool {
        matches!(self, Self::Object(_) | Self::Array(_))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    pub fn width(&self) -> u32 {
        if self.is_wide() { 2 } else { 1 }
    }

    /// Parses a descriptor that is already known to be well formed.
    pub fn from_descriptor(descriptor: &str) -> Self {
        descriptor.parse().unwrap_or(Self::Unknown(Vec::new()))
    }

    /// The Java source name of this type, e.g. `String` or `int[]`.
    ///
    /// This is the renderer's own conversion, so a parameter type and a type the
    /// renderer derives from a descriptor are spelled the same way.
    pub fn java_name(&self) -> String {
        radguard::backend::descriptor_to_java(&self.descriptor())
    }

    /// The descriptor this type renders as.
    pub fn descriptor(&self) -> Cow<'_, str> {
        match self {
            Self::Void => Cow::Borrowed("V"),
            Self::Primitive(primitive) => Cow::Borrowed(primitive.descriptor()),
            Self::Object(name) => Cow::Owned(format!("L{name};")),
            Self::Array(element) => Cow::Owned(format!("[{}", element.descriptor())),
            Self::Unknown(_) => Cow::Borrowed("Ljava/lang/Object;"),
        }
    }

    /// Work-budget cost: proportional to how much the type costs to carry.
    pub fn cost(&self) -> usize {
        match self {
            Self::Object(name) => 1 + name.len(),
            Self::Array(element) => 1 + element.cost(),
            Self::Unknown(choices) => 1 + choices.len(),
            _ => 1,
        }
    }

    pub(crate) fn integral(&self) -> bool {
        match self {
            Self::Primitive(primitive) => primitive.is_integral(),
            Self::Unknown(choices) => {
                !choices.is_empty() && choices.iter().all(|p| p.is_integral())
            }
            _ => false,
        }
    }
}

/// A parsed method descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodDescriptor {
    pub parameters: Vec<Type>,
    pub return_type: Type,
}

impl MethodDescriptor {
    pub fn parse(descriptor: &str) -> Result<Self> {
        let mut parser = Parser::new(descriptor)?;
        parser.expect('(')?;
        let mut parameters = Vec::new();
        while parser.peek() != Some(')') {
            let ty = parser.parse_type()?;
            ensure!(
                !ty.is_void(),
                "INVALID_INPUT: void method parameter in {descriptor}"
            );
            parameters.push(ty);
        }
        parser.expect(')')?;
        let return_type = parser.parse_type()?;
        parser.finish()?;
        ensure!(
            parameters
                .iter()
                .map(|ty| ty.width() as usize)
                .sum::<usize>()
                <= 255,
            "INVALID_INPUT: method parameter word limit exceeded"
        );
        Ok(Self {
            parameters,
            return_type,
        })
    }
}

impl std::str::FromStr for Type {
    type Err = anyhow::Error;

    fn from_str(descriptor: &str) -> Result<Self, Self::Err> {
        field_type(descriptor)
    }
}

/// Parses one field descriptor.
pub fn field_type(descriptor: &str) -> Result<Type> {
    let mut parser = Parser::new(descriptor)?;
    let ty = parser.parse_type()?;
    parser.finish()?;
    ensure!(!ty.is_void(), "INVALID_INPUT: void field type {descriptor}");
    Ok(ty)
}

/// Parses a class name or array descriptor into its type.
pub fn type_for_class(name: &str) -> Result<Type> {
    if name.starts_with('[') {
        field_type(name)
    } else {
        ensure!(
            !name.is_empty() && name.len() <= MAX_DESCRIPTOR,
            "INVALID_INPUT: invalid class name"
        );
        Ok(Type::object(name))
    }
}

struct Parser<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_DESCRIPTOR,
            "RESOURCE_LIMIT: descriptor length"
        );
        ensure!(!text.is_empty(), "INVALID_INPUT: empty descriptor");
        Ok(Self { text, position: 0 })
    }

    fn peek(&self) -> Option<char> {
        self.text[self.position..].chars().next()
    }

    fn next(&mut self) -> Result<char> {
        let value = self.peek().context("INVALID_INPUT: truncated descriptor")?;
        self.position += value.len_utf8();
        Ok(value)
    }

    fn expect(&mut self, expected: char) -> Result<()> {
        let value = self.next()?;
        ensure!(
            value == expected,
            "INVALID_INPUT: expected {expected} in {}",
            self.text
        );
        Ok(())
    }

    fn finish(&self) -> Result<()> {
        ensure!(
            self.position == self.text.len(),
            "INVALID_INPUT: trailing characters in {}",
            self.text
        );
        Ok(())
    }

    fn parse_type(&mut self) -> Result<Type> {
        let mut rank = 0;
        while self.peek() == Some('[') {
            self.position += 1;
            rank += 1;
            ensure!(rank <= 255, "INVALID_INPUT: array rank exceeds 255");
        }
        let value = self.next()?;
        let mut ty = match Primitive::from_char(value) {
            Some(primitive) => Type::Primitive(primitive),
            None => match value {
                'V' => Type::Void,
                'L' => {
                    let start = self.position;
                    while self.peek().is_some_and(|value| value != ';') {
                        self.position += 1;
                    }
                    let name = &self.text[start..self.position];
                    ensure!(
                        !name.is_empty() && !name.starts_with('['),
                        "INVALID_INPUT: invalid class name in {}",
                        self.text
                    );
                    self.expect(';')?;
                    Type::Object(name.to_owned())
                }
                other => bail!("INVALID_INPUT: unknown descriptor character {other}"),
            },
        };
        for _ in 0..rank {
            ensure!(
                !matches!(ty, Type::Void),
                "INVALID_INPUT: void array element in {}",
                self.text
            );
            ty = Type::Array(Box::new(ty));
        }
        Ok(ty)
    }
}

/// A decoded JVM string.
///
/// The constant pool stores UTF-16 code units, and a Java string may hold an
/// unpaired surrogate that no `String` can represent, so the units are kept and
/// rendered lossily on the way out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utf16String {
    units: Vec<u16>,
    text: String,
}

impl Utf16String {
    pub fn from_utf16(units: Vec<u16>) -> Self {
        let text = String::from_utf16_lossy(&units);
        Self { units, text }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The raw code units, for callers that must report a lossy decode.
    pub fn as_utf16(&self) -> &[u16] {
        &self.units
    }

    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_method_descriptors() {
        let method = MethodDescriptor::parse("(ILjava/lang/String;[J)D").expect("descriptor");
        assert_eq!(
            method.parameters,
            vec![Type::INT, Type::string(), Type::array(Type::LONG)]
        );
        assert_eq!(method.return_type, Type::DOUBLE);
        assert_eq!(Type::array(Type::INT).descriptor(), "[I");
        assert_eq!(Type::string().descriptor(), "Ljava/lang/String;");
    }

    #[test]
    fn rejects_malformed_descriptors() {
        for descriptor in ["", "(I", "(V)V", "I)", "L;", "[", "(I)"] {
            assert!(
                MethodDescriptor::parse(descriptor).is_err(),
                "{descriptor} should not parse as a method"
            );
        }
        assert!(field_type("V").is_err());
        assert!(field_type("I").is_ok());
    }

    #[test]
    fn decodes_wide_strings() {
        let units: Vec<u16> = "前\u{0}\u{1f600}".encode_utf16().collect();
        assert_eq!(Utf16String::from_utf16(units).as_str(), "前\u{0}😀");
    }
}
