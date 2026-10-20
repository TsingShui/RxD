//! JVM metadata and stack bytecode lifted into the backend IR, without a JVM or D8.
mod classfile;
mod code;
mod input;
mod opcodes;
mod types;

pub use classfile::{ClassFile, Code, Field, Handler, Local, MemberKind, Method, Reference};
pub use code::{Instruction, bytecode, decode_method};
pub use input::{Input, InputClass, normalize_class_name};

use radguard::backend::{
    BlockKind, ClassSpec, Constant, ConstantValue, FieldSpec, InvokeExpression, InvokeKind,
    LoweredBlock, LoweredMethod, MethodReference, MethodSpec, Statement, Value, VariableTable,
    render_class as render_class_spec, render_method as render_method_spec,
};

/// Rendered Java modifiers of a class.
fn class_modifiers(access: u16, is_interface: bool) -> Vec<String> {
    let mut modifiers = Vec::new();
    for (flag, name) in [
        (0x0001, "public"),
        (0x0010, "final"),
        // An interface is implicitly abstract; the DEX path drops it too.
        (0x0400, "abstract"),
    ] {
        if access & flag != 0 && !(is_interface && name == "abstract") {
            modifiers.push(name.to_owned());
        }
    }
    modifiers
}

/// Rendered Java modifiers of a method, in declaration order.
fn method_modifiers(access: u16) -> Vec<String> {
    let mut modifiers = Vec::new();
    for (flag, name) in [
        (0x0001, "public"),
        (0x0004, "protected"),
        (0x0002, "private"),
        (0x0400, "abstract"),
        (0x0008, "static"),
        (0x0010, "final"),
        (0x0020, "synchronized"),
        (0x0100, "native"),
        (0x0800, "strictfp"),
    ] {
        if access & flag != 0 {
            modifiers.push(name.to_owned());
        }
    }
    modifiers
}

/// Rendered Java modifiers of a field, in declaration order.
fn field_modifiers(access: u16) -> Vec<String> {
    let mut modifiers = Vec::new();
    for (flag, name) in [
        (0x0001, "public"),
        (0x0004, "protected"),
        (0x0002, "private"),
        (0x0008, "static"),
        (0x0010, "final"),
        (0x0040, "volatile"),
        (0x0080, "transient"),
    ] {
        if access & flag != 0 {
            modifiers.push(name.to_owned());
        }
    }
    modifiers
}

/// The class descriptor of a parsed class.
fn descriptor_of(class: &ClassFile) -> String {
    format!("L{};", class.name)
}

/// The simple name a constructor displays, without package or nesting marker.
fn simple_name(class: &ClassFile) -> String {
    class
        .name
        .rsplit('/')
        .next()
        .unwrap_or(class.name.as_str())
        .to_owned()
}

/// The declared shape of one method.
fn method_spec(method: &Method) -> anyhow::Result<MethodSpec> {
    let proto = classfile::method_type(&method.descriptor)?;
    Ok(MethodSpec {
        modifiers: method_modifiers(method.access),
        is_constructor: method.name == "<init>",
        return_type: proto.return_type.java_name(),
        name: method.name.clone(),
        descriptor: method.descriptor.clone(),
        parameter_types: proto
            .parameters
            .iter()
            .map(crate::types::Type::java_name)
            .collect(),
    })
}

/// The body a method gets when its bytecode cannot be lowered.
///
/// This is the honest degradation the DEX `get-class` path documents: the method
/// is present, says so, and throws when called, instead of rendering an empty
/// body that would silently read as if it did nothing.
fn stub_body(message: &str) -> LoweredMethod {
    let exception = "Ljava/lang/UnsupportedOperationException;".to_owned();
    let value = Value::Invoke(Box::new(InvokeExpression {
        kind: InvokeKind::Direct,
        method: MethodReference {
            class: exception.clone(),
            name: "<init>".to_owned(),
            parameters: vec!["Ljava/lang/String;".to_owned()],
            return_type: "V".to_owned(),
        },
        base: Value::NewInstance { ty: exception },
        arguments: vec![Value::Constant(Constant {
            value: ConstantValue::String(message.to_owned()),
        })],
    }));
    LoweredMethod {
        variables: VariableTable::default(),
        blocks: vec![LoweredBlock {
            start: 0,
            end: 0,
            kind: BlockKind::Throw,
            statements: vec![Statement::Throw(value)],
            predecessors: Vec::new(),
            successors: Vec::new(),
            exception_successors: Vec::new(),
            exception_handlers: Vec::new(),
            branch_true: None,
            branch_false: None,
            exception: None,
        }],
    }
}

/// Decodes one method body.
///
/// `get-method` asks for exactly one body, so a body that cannot be lowered is
/// the answer: the diagnostic travels out (`UNSUPPORTED_BYTECODE`, a resource
/// limit, a malformed body) and no guessed Java is produced.
fn method_body(
    class: &ClassFile,
    method: &Method,
    limits: Limits,
) -> anyhow::Result<Option<LoweredMethod>> {
    if method.code.is_none() {
        return Ok(None);
    }
    Ok(Some(decode_method(class, method, limits)?))
}

/// Decodes one method body for whole-class rendering, where an undecodable body
/// degrades to a throwing stub instead of failing the class.
fn class_method_body(
    class: &ClassFile,
    method: &Method,
    limits: Limits,
) -> anyhow::Result<Option<LoweredMethod>> {
    if method.code.is_none() {
        return Ok(None);
    }
    Ok(Some(match decode_method(class, method, limits) {
        Ok(body) => body,
        Err(error) => stub_body(&format!("{error:#}")),
    }))
}

pub fn render_method(input: &Input, coordinate: &str, limits: Limits) -> anyhow::Result<String> {
    let (selected, index) = input.method_by_at(coordinate)?;
    let class = &selected.class;
    let method = &class.methods[index];
    let spec = method_spec(method)?;
    let body = method_body(class, method, limits)?;
    let output = render_method_spec(&descriptor_of(class), &simple_name(class), &spec, body);
    Ok(output.source)
}

/// Decompile a whole class to Java.
///
/// Every field and method is assembled into one class; a method whose bytecode
/// cannot be lowered degrades to a throwing stub, and one without a Code
/// attribute renders as a bodyless declaration.
pub fn render_class(input: &Input, name: &str, limits: Limits) -> anyhow::Result<String> {
    let selected = input.class_by_name(name)?;
    let class = &selected.class;
    let is_interface = class.access & 0x0200 != 0;
    let mut methods = Vec::with_capacity(class.methods.len());
    for method in &class.methods {
        methods.push((
            method_spec(method)?,
            class_method_body(class, method, limits)?,
        ));
    }
    let mut fields = Vec::with_capacity(class.fields.len());
    for field in &class.fields {
        let ty = classfile::field_type(&field.descriptor)?;
        fields.push(
            FieldSpec::new(ty.java_name(), field.name.clone(), field.descriptor.clone())
                .with_modifiers(field_modifiers(field.access)),
        );
    }
    let spec = ClassSpec {
        descriptor: descriptor_of(class),
        package: class
            .name
            .rsplit_once('/')
            .map(|(package, _)| package.replace('/', "."))
            .unwrap_or_default(),
        name: simple_name(class),
        modifiers: class_modifiers(class.access, is_interface),
        is_interface,
        super_class: class
            .super_name
            .as_deref()
            .filter(|name| *name != "java/lang/Object")
            .map(|name| name.replace('/', ".")),
        interfaces: class
            .interfaces
            .iter()
            .map(|name| name.replace('/', "."))
            .collect(),
        fields,
        methods,
    };
    Ok(render_class_spec(spec).source)
}

/// A `findrefs` query: which kind of reference to search for.
///
/// `string`/`type` match by case-sensitive substring over the literal/type
/// name. `method`/`field` match the member name exactly, optionally constrained
/// to an owner class by case-sensitive substring (`class`).
pub enum RefQuery<'a> {
    String(RefPattern<'a>),
    Type(RefPattern<'a>),
    Method {
        name: RefPattern<'a>,
        class: Option<RefPattern<'a>>,
        /// The prototype of an exact coordinate anchor, e.g. `(I)V`; `None` for a search.
        descriptor: Option<&'a str>,
    },
    Field {
        name: RefPattern<'a>,
        class: Option<RefPattern<'a>>,
        /// The type of an exact coordinate anchor, e.g. `I`; `None` for a search.
        descriptor: Option<&'a str>,
    },
}

/// One selector face: a content substring, or an exact identity.
///
/// `-f/--filter` asks whether the value contains the needle; `--at` asks whether the value
/// *is* the identity. Both are case-sensitive, and the difference is the whole answer: a
/// search that matches nothing answers with an empty result, an anchor that resolves nowhere
/// is an error.
#[derive(Clone, Copy, Debug)]
pub struct RefPattern<'a> {
    pub text: &'a str,
    pub exact: bool,
    /// Whether the substring search folds case. A coordinate is matched exactly, so this
    /// only governs the `-f` face; the CLI's `--case` decides it.
    pub case_sensitive: bool,
}

impl<'a> RefPattern<'a> {
    pub fn search(text: &'a str, case_sensitive: bool) -> Self {
        Self {
            text,
            exact: false,
            case_sensitive,
        }
    }

    pub fn coordinate(text: &'a str) -> Self {
        Self {
            text,
            exact: true,
            case_sensitive: true,
        }
    }

    fn matches(&self, value: &str) -> bool {
        let compare = rxd_core::filter::Contains::new(self.text, self.case_sensitive);
        if self.exact {
            rxd_core::filter::equals(value, self.text, self.case_sensitive)
        } else {
            compare.matches(value)
        }
    }
}

impl RefQuery<'_> {
    /// Whether this query came from an `--at` coordinate rather than a content search.
    pub fn is_anchor(&self) -> bool {
        match self {
            Self::String(pattern) | Self::Type(pattern) => pattern.exact,
            Self::Method { descriptor, .. } | Self::Field { descriptor, .. } => {
                descriptor.is_some()
            }
        }
    }

    /// The error text for an anchor whose identity no class carries.
    pub fn anchor_not_found(&self) -> String {
        match self {
            Self::String(pattern) => format!("no string literal is exactly {:?}", pattern.text),
            Self::Type(pattern) => format!("no type is named {}", pattern.text),
            Self::Method {
                name,
                class,
                descriptor,
            } => format!(
                "no method {}{} matches {}",
                name.text,
                descriptor.unwrap_or_default(),
                class.map_or("the input".to_owned(), |class| class.text.to_owned())
            ),
            Self::Field {
                name,
                class,
                descriptor,
            } => format!(
                "no field {}:{} matches {}",
                name.text,
                descriptor.unwrap_or_default(),
                class.map_or("the input".to_owned(), |class| class.text.to_owned())
            ),
        }
    }
}

/// One `findrefs` hit, carrying the owning class and method so a reader can
/// route to `get-method`/`get-class` (ADR 0002 closure).
#[derive(serde::Serialize)]
pub struct RefHit {
    /// Content-bound FETCH context for the referencing instruction.
    /// `Lclass;->name(proto)`: the caller's coordinate, printable and selectable.
    pub method_at: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    pub class: String,
    pub method: String,
    pub method_descriptor: String,
    pub byte_offset: u32,
    pub mnemonic: &'static str,
    /// Human-routable description of the referenced target.
    pub target: String,
}

/// Scan every selected class's method bodies for references matching `query`.
///
/// String literals are matched directly from each method's `ldc` operands; type
/// and member references are matched from the resolved constant-pool entry of
/// each referencing instruction. Hits are returned in (class, method, offset)
/// source order.
pub fn find_refs(input: &Input, query: &RefQuery, limits: Limits) -> anyhow::Result<Vec<RefHit>> {
    let mut hits = Vec::new();
    let mut work = 0usize;
    for selected in &input.classes {
        let class = &selected.class;
        for (method_index, method) in class.methods.iter().enumerate() {
            if method.code.is_none() {
                continue;
            }
            for insn in bytecode(class, method)? {
                work += 1;
                anyhow::ensure!(
                    work <= limits.work,
                    "RESOURCE_LIMIT: scan work budget exceeded"
                );
                let Some(index) = insn.constant_index else {
                    continue;
                };
                let Some(target) = ref_target(class, index, query) else {
                    continue;
                };
                hits.push(RefHit {
                    method_at: Input::method_at(selected, method_index),
                    source_id: selected.source_id.clone(),
                    entry: selected.entry.clone(),
                    class: class.name.clone(),
                    method: method.name.clone(),
                    method_descriptor: method.descriptor.clone(),
                    byte_offset: insn.byte_offset,
                    mnemonic: insn.mnemonic,
                    target,
                });
                anyhow::ensure!(
                    hits.len() <= limits.entries,
                    "RESOURCE_LIMIT: record budget exceeded"
                );
            }
        }
    }
    Ok(hits)
}

/// Match one instruction's constant-pool reference against the query, returning
/// a routable target description on a hit.
fn ref_target(class: &ClassFile, index: u16, query: &RefQuery) -> Option<String> {
    match query {
        RefQuery::String(pattern) => {
            // Only `ldc`/`ldc_w` (0x12/0x13) load a String constant.
            let literal = class.string(index).ok()?;
            // The literal prints as the bytes it is, so `--at` takes the same text back.
            pattern
                .matches(literal.as_str())
                .then(|| literal.as_str().to_owned())
        }
        RefQuery::Type(pattern) => match class.reference_at(index)? {
            // A type operand already prints as a descriptor; a member operand names its owner
            // in internal form, so both are compared in the descriptor spelling a row prints.
            Reference::Type { name } => pattern.matches(&name).then_some(name),
            Reference::Member { owner, .. } => {
                let descriptor = format!("L{owner};");
                pattern.matches(&descriptor).then_some(descriptor)
            }
        },
        RefQuery::Method {
            name,
            class: owner_filter,
            descriptor: wanted,
        } => match class.reference_at(index)? {
            Reference::Member {
                kind: MemberKind::Method,
                owner,
                name: member,
                descriptor,
            } if name.matches(&member)
                && wanted.is_none_or(|wanted| wanted == descriptor)
                && owner_filter.is_none_or(|pattern| pattern.matches(&owner)) =>
            {
                // `Lowner;->name(proto)` is what `get-method --at` takes, so a hit routes
                // straight back into the decompiler.
                Some(format!("L{owner};->{member}{descriptor}"))
            }
            _ => None,
        },
        RefQuery::Field {
            name,
            class: owner_filter,
            descriptor: wanted,
        } => match class.reference_at(index)? {
            Reference::Member {
                kind: MemberKind::Field,
                owner,
                name: member,
                descriptor,
            } if name.matches(&member)
                && wanted.is_none_or(|wanted| wanted == descriptor)
                && owner_filter.is_none_or(|pattern| pattern.matches(&owner)) =>
            {
                Some(format!("L{owner};->{member}:{descriptor}"))
            }
            _ => None,
        },
    }
}

/// Whether the input carries the query's anchor at all.
///
/// A coordinate names one identity, and "no class carries it" is a different answer from "no
/// instruction references it": the first is an error naming what was looked for, the second is
/// a true empty result. An identity counts as carried when a class declares it or when some
/// constant pool holds it, which is what lets an external member (a `StringBuilder.append`
/// call) be anchored even though no class in the input declares it.
pub fn resolves(input: &Input, query: &RefQuery) -> bool {
    if !query.is_anchor() {
        return true;
    }
    input.classes.iter().any(|selected| {
        let class = &selected.class;
        carries(class, query)
    })
}

fn carries(class: &ClassFile, query: &RefQuery) -> bool {
    match query {
        RefQuery::String(pattern) => class
            .string_constants()
            .iter()
            .any(|(_, literal)| pattern.matches(literal.as_str())),
        RefQuery::Type(pattern) => {
            class
                .reference_at_index_iter()
                .any(|reference| match reference {
                    Reference::Type { name } => pattern.matches(&name),
                    Reference::Member { owner, .. } => pattern.matches(&format!("L{owner};")),
                })
                || pattern.matches(&format!("L{};", class.name))
        }
        RefQuery::Method {
            name,
            class: owner_filter,
            descriptor,
        } => {
            declares_member(class, name, owner_filter, descriptor, true)
                || pool_member(class, name, owner_filter, descriptor, true)
        }
        RefQuery::Field {
            name,
            class: owner_filter,
            descriptor,
        } => {
            declares_member(class, name, owner_filter, descriptor, false)
                || pool_member(class, name, owner_filter, descriptor, false)
        }
    }
}

fn declares_member(
    class: &ClassFile,
    name: &RefPattern<'_>,
    owner_filter: &Option<RefPattern<'_>>,
    descriptor: &Option<&str>,
    methods: bool,
) -> bool {
    if owner_filter.is_some_and(|pattern| !pattern.matches(&class.name)) {
        return false;
    }
    let matches = |member_name: &str, member_descriptor: &str| {
        name.matches(member_name) && descriptor.is_none_or(|wanted| wanted == member_descriptor)
    };
    if methods {
        class
            .methods
            .iter()
            .any(|method| matches(&method.name, &method.descriptor))
    } else {
        class
            .fields
            .iter()
            .any(|field| matches(&field.name, &field.descriptor))
    }
}

/// A member the class *references* but may not declare: an external call.
fn pool_member(
    class: &ClassFile,
    name: &RefPattern<'_>,
    owner_filter: &Option<RefPattern<'_>>,
    descriptor: &Option<&str>,
    methods: bool,
) -> bool {
    class
        .reference_at_index_iter()
        .any(|reference| match reference {
            Reference::Member {
                kind,
                owner,
                name: member,
                descriptor: member_descriptor,
            } => {
                (kind == MemberKind::Method) == methods
                    && name.matches(&member)
                    && descriptor.is_none_or(|wanted| wanted == member_descriptor)
                    && owner_filter.is_none_or(|pattern| pattern.matches(&owner))
            }
            Reference::Type { .. } => false,
        })
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub input_bytes: usize,
    pub class_bytes: usize,
    pub expanded_bytes: usize,
    pub entries: usize,
    pub work: usize,
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        let budget = rxd_core::Budget::default();
        Self {
            input_bytes: budget.input_bytes,
            class_bytes: budget.entry_bytes,
            expanded_bytes: budget.metadata_bytes,
            entries: budget.entries,
            work: budget.work,
            output_bytes: budget.output_bytes,
        }
    }
}
