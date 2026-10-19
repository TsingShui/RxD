use crate::dex::{ClassDef, DexFile, EncodedAnnotation, EncodedMethod, EncodedValue};
use crate::error::{Error, Result};
use crate::ir::opcode::lower_method_with_resolved;
use crate::types::{class_access, descriptor_to_java, field_access, method_access};
use crate::writer::{SourceOutput, escape_java_string};

/// A parsed DEX file that can render Java-like source per class.
///
/// Constructing one parses the DEX once; decompiling a class never touches the
/// file system, spawns no threads and keeps no global state, so a decompiler is
/// safe to keep around and to share behind `&self`.
///
/// Malformed DEX input is reported as [`Error`]; no DEX input is expected to
/// panic the process.
pub struct Decompiler<'a> {
    dex: DexFile<'a>,
    include_metadata: bool,
}

impl<'a> Decompiler<'a> {
    /// Parses DEX bytes borrowed from the caller.
    pub fn parse(dex_bytes: &'a [u8]) -> Result<Self> {
        Ok(Self {
            dex: DexFile::parse(dex_bytes)?,
            include_metadata: true,
        })
    }

    /// Parses DEX bytes owned by a shared buffer.
    ///
    /// The returned decompiler borrows nothing, which lets callers share one
    /// DEX allocation across threads instead of copying it per worker.
    pub fn from_arc(dex_bytes: std::sync::Arc<[u8]>) -> Result<Decompiler<'static>> {
        Ok(Decompiler {
            dex: DexFile::parse_owned(dex_bytes)?,
            include_metadata: true,
        })
    }

    /// Disables DEX annotations in rendered source while retaining member-reference metadata.
    pub const fn without_metadata(mut self) -> Self {
        self.include_metadata = false;
        self
    }

    /// The parsed DEX tables behind this decompiler.
    pub fn dex(&self) -> &DexFile<'a> {
        &self.dex
    }

    /// Renders one class as Java-like source.
    ///
    /// A descriptor the DEX does not define, an empty descriptor and a
    /// malformed descriptor all answer `Ok(None)` rather than an error, so
    /// callers can tell "not in this DEX" apart from "decompilation failed".
    pub fn decompile_class(&self, dalvik_class: &str) -> Result<Option<SourceOutput>> {
        let Some(class) = self.dex.class(dalvik_class) else {
            return Ok(None);
        };
        self.write_class(class).map(Some)
    }

    /// Renders one method of one class as Java-like source.
    ///
    /// Matching is by name and prototype because that pair is the method's identity
    /// inside its class; a class or method the DEX does not define answers `Ok(None)`.
    pub fn decompile_method(
        &self,
        dalvik_class: &str,
        name: &str,
        prototype: &str,
    ) -> Result<Option<SourceOutput>> {
        let Some(class) = self.dex.class(dalvik_class) else {
            return Ok(None);
        };
        let descriptor = self.dex.class_descriptor(class)?;
        let (_, class_name) = class_parts(descriptor);
        for method in class.methods() {
            let resolved = self.dex.method(method.method_idx)?;
            if resolved.name == name && resolved.descriptor == prototype {
                return self
                    .write_method(
                        SourceOutput::default(),
                        descriptor,
                        &class_name,
                        method,
                        &[],
                        false,
                    )
                    .map(Some);
            }
        }
        Ok(None)
    }

    pub fn decompile_all(&self) -> Result<Vec<(String, SourceOutput)>> {
        self.dex
            .classes()
            .iter()
            .map(|class| {
                Ok((
                    self.dex.class_descriptor(class)?.to_owned(),
                    self.write_class(class)?,
                ))
            })
            .collect()
    }

    /// Decompile one scheduler-owned class range. APK pipeline workers use this
    /// primitive so every DEX shares one global class-worker pool rather than
    /// creating nested pools per DEX.
    pub(crate) fn decompile_class_range_for_export<F>(
        &self,
        range: std::ops::Range<usize>,
        mut visit: F,
    ) -> Result<usize>
    where
        F: FnMut(&str, SourceOutput) -> Result<()>,
    {
        let classes = self
            .dex
            .classes()
            .get(range.clone())
            .ok_or_else(|| Error::data(0, "APK class job has an invalid class range"))?;
        for class in classes {
            let descriptor = self.dex.class_descriptor(class)?;
            let output = self.write_class(class)?;
            visit(descriptor, output)?;
        }
        Ok(range.len())
    }

    pub fn decompile_all_parallel(
        &self,
        thread_count: usize,
    ) -> Result<Vec<(String, SourceOutput)>> {
        let classes = self.dex.classes();
        if thread_count <= 1 || classes.len() <= 1 {
            return self.decompile_all();
        }

        let worker_count = thread_count.min(classes.len());
        let chunk_size = classes.len().div_ceil(worker_count);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for chunk in classes.chunks(chunk_size) {
                handles.push(scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|class| {
                            Ok((
                                self.dex.class_descriptor(class)?.to_owned(),
                                self.write_class(class)?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()
                }));
            }

            let mut output = Vec::with_capacity(classes.len());
            for handle in handles {
                let chunk = handle
                    .join()
                    .map_err(|_| Error::data(0, "parallel decompiler worker panicked"))??;
                output.extend(chunk);
            }
            Ok(output)
        })
    }

    fn write_class(&self, class: &ClassDef) -> Result<SourceOutput> {
        self.write_class_into(SourceOutput::default(), class)
    }

    fn write_class_into(&self, mut output: SourceOutput, class: &ClassDef) -> Result<SourceOutput> {
        let descriptor = self.dex.class_descriptor(class)?;
        let (package, name) = class_parts(descriptor);
        let interface = class.access_flags & 0x200 != 0;
        let access_flags = if interface {
            class.access_flags & !0x400
        } else {
            class.access_flags
        };
        let super_class = match self.dex.class_superclass(class)? {
            Some(superclass) if superclass != "Ljava/lang/Object;" => Some(java_name(superclass)),
            _ => None,
        };
        let interfaces = self
            .dex
            .class_interfaces(class)?
            .into_iter()
            .map(java_name)
            .collect();
        crate::render::write_class_header(
            &mut output,
            &crate::render::ClassHeader {
                package,
                modifiers: class_access(access_flags),
                is_interface: interface,
                name: name.clone(),
                super_class,
                interfaces,
            },
        );

        for field in class.fields() {
            let resolved = self.dex.field(field.field_idx)?;
            let constant = field
                .initial_value
                .as_ref()
                .map(|value| self.encoded_value(value, resolved.ty))
                .transpose()?;
            crate::render::write_field(
                &mut output,
                resolved.class,
                &crate::render::FieldHeader {
                    modifiers: field_access(field.access_flags),
                    ty: descriptor_to_java(resolved.ty),
                    name: resolved.name.to_owned(),
                    descriptor: resolved.ty.to_owned(),
                    constant,
                },
            );
        }
        let method_annotations = if self.include_metadata {
            Some(self.dex.method_annotations(class)?)
        } else {
            None
        };
        for method in class.methods() {
            let annotations = method_annotations
                .as_ref()
                .and_then(|annotations| annotations.get(&method.method_idx))
                .map(Vec::as_slice)
                .unwrap_or_default();
            output = self.write_method(output, descriptor, &name, method, annotations, true)?;
        }
        output.source.push_str("}\n");
        Ok(output)
    }

    fn write_method(
        &self,
        output: SourceOutput,
        class_descriptor: &str,
        class_name: &str,
        method: &EncodedMethod,
        annotations: &[EncodedAnnotation],
        indent: bool,
    ) -> Result<SourceOutput> {
        let resolved = self.dex.method(method.method_idx)?;
        let access = method_access(method.access_flags);
        let is_constructor = access.iter().any(|value| value == "constructor");
        let modifiers: Vec<String> = access
            .iter()
            .filter(|value| value.as_str() != "constructor")
            .cloned()
            .collect();
        let mut rendered_annotations = Vec::with_capacity(annotations.len());
        for annotation in annotations {
            rendered_annotations.push(self.annotation(annotation)?);
        }
        let mut parameter_types = Vec::with_capacity(resolved.parameter_type_indices.len());
        for index in resolved.parameter_type_indices {
            parameter_types.push(descriptor_to_java(self.dex.type_descriptor(*index)?));
        }
        let body = lower_method_with_resolved(&self.dex, method, resolved)?;
        let header = crate::render::MethodHeader {
            modifiers,
            is_constructor,
            return_type: descriptor_to_java(resolved.return_type),
            name: resolved.name.to_owned(),
            descriptor: resolved.descriptor.to_owned(),
            parameter_types,
            annotations: rendered_annotations,
            style: crate::render::SourceStyle::DAD,
            indent,
        };
        Ok(crate::render::write_method(
            output,
            class_descriptor,
            class_name,
            &header,
            body,
        ))
    }

    fn encoded_value(&self, value: &EncodedValue, field_type: &str) -> Result<String> {
        let rendered = match value {
            EncodedValue::Byte(value) => {
                if field_type == "B" {
                    signed_hex(i64::from(*value))
                } else {
                    value.to_string()
                }
            }
            EncodedValue::Short(value) => value.to_string(),
            EncodedValue::Char(value) => value.to_string(),
            EncodedValue::Int(value) => value.to_string(),
            EncodedValue::Long(value) => value.to_string(),
            EncodedValue::Float(bits) => f32::from_bits(*bits).to_string(),
            EncodedValue::Double(bits) => f64::from_bits(*bits).to_string(),
            EncodedValue::String(index) => escape_java_string(self.dex.string(*index)?),
            EncodedValue::Type(index) => {
                format!(
                    "{}.class",
                    descriptor_to_java(self.dex.type_descriptor(*index)?)
                )
            }
            EncodedValue::Field(index) | EncodedValue::Enum(index) => {
                let field = self.dex.field(*index)?;
                format!("{}.{}", descriptor_to_java(field.class), field.name)
            }
            EncodedValue::Method(index) => {
                let method = self.dex.method(*index)?;
                format!("{}.{}", descriptor_to_java(method.class), method.name)
            }
            EncodedValue::MethodType(index) => self.dex.proto_descriptor(*index)?,
            EncodedValue::MethodHandle(index) => index.to_string(),
            EncodedValue::Array(values) => {
                let values = values
                    .iter()
                    .map(|value| self.encoded_value(value, field_type))
                    .collect::<Result<Vec<_>>>()?;
                format!("{{{}}}", values.join(", "))
            }
            EncodedValue::Annotation(annotation) => {
                let name = descriptor_to_java(self.dex.type_descriptor(annotation.type_idx)?);
                let mut elements = Vec::with_capacity(annotation.elements.len());
                for element in &annotation.elements {
                    elements.push(format!(
                        "{}={}",
                        self.dex.string(element.name_idx)?,
                        self.encoded_value(&element.value, field_type)?
                    ));
                }
                if elements.is_empty() {
                    format!("@{name}")
                } else {
                    format!("@{name}({})", elements.join(", "))
                }
            }
            EncodedValue::Null if field_type == "Ljava/lang/String;" => "\"\"".to_owned(),
            EncodedValue::Null => "None".to_owned(),
            EncodedValue::Boolean(value) => {
                if *value {
                    "True".to_owned()
                } else {
                    "False".to_owned()
                }
            }
        };
        Ok(rendered)
    }

    fn annotation(&self, annotation: &EncodedAnnotation) -> Result<String> {
        let name = annotation_type_name(self.dex.type_descriptor(annotation.type_idx)?);
        if annotation.elements.is_empty() {
            return Ok(format!("@{name}"));
        }
        let mut elements = Vec::with_capacity(annotation.elements.len());
        for element in &annotation.elements {
            elements.push(format!(
                "{}={}",
                self.dex.string(element.name_idx)?,
                self.annotation_value(&element.value)?
            ));
        }
        Ok(format!("@{name}({})", elements.join(", ")))
    }

    fn annotation_value(&self, value: &EncodedValue) -> Result<String> {
        Ok(match value {
            EncodedValue::String(index) => escape_java_string(self.dex.string(*index)?),
            EncodedValue::Type(index) => format!(
                "{}.class",
                annotation_type_name(self.dex.type_descriptor(*index)?)
            ),
            EncodedValue::Array(values) => {
                let values = values
                    .iter()
                    .map(|value| self.annotation_value(value))
                    .collect::<Result<Vec<_>>>()?;
                format!("{{{}}}", values.join(", "))
            }
            EncodedValue::Annotation(annotation) => self.annotation(annotation)?,
            EncodedValue::Boolean(value) => {
                if *value {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
            EncodedValue::Null => "null".to_owned(),
            _ => self.encoded_value(value, "")?,
        })
    }
}

pub fn decompile_dex_bytes(dex_bytes: &[u8], dalvik_class_fmt: &str) -> Result<SourceOutput> {
    let decompiler = Decompiler::parse(dex_bytes)?;
    Ok(match decompiler.decompile_class(dalvik_class_fmt)? {
        Some(output) => output,
        None => SourceOutput {
            source: format!("Error: Class {dalvik_class_fmt} not found in the reconstructed DEX."),
            member_references: Vec::new(),
        },
    })
}

/// A Dalvik descriptor as a dotted Java name: `Ljava/lang/String;` -> `java.lang.String`.
fn java_name(descriptor: &str) -> String {
    descriptor
        .get(1..descriptor.len().saturating_sub(1))
        .unwrap_or(descriptor)
        .replace('/', ".")
}

fn class_parts(descriptor: &str) -> (String, String) {
    let body = descriptor
        .strip_prefix('L')
        .and_then(|value| value.strip_suffix(';'))
        .unwrap_or(descriptor);
    if let Some((package, name)) = body.rsplit_once('/') {
        (package.replace('/', "."), name.to_owned())
    } else {
        (String::new(), body.to_owned())
    }
}

fn signed_hex(value: i64) -> String {
    if value < 0 {
        format!("-0x{:x}", value.unsigned_abs())
    } else {
        format!("0x{value:x}")
    }
}

fn annotation_type_name(descriptor: &str) -> String {
    descriptor
        .strip_prefix('L')
        .and_then(|value| value.strip_suffix(';'))
        .unwrap_or(descriptor)
        .replace('/', ".")
        .rsplit_once('.')
        .map(|(_, name)| name)
        .unwrap_or(descriptor)
        .replace('$', ".")
}
