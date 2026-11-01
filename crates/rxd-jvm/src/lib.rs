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

fn descriptor_of(class: &ClassFile) -> String {
    format!("L{};", class.name)
}

fn simple_name(class: &ClassFile) -> String {
    class
        .name
        .rsplit('/')
        .next()
        .unwrap_or(class.name.as_str())
        .to_owned()
}

fn method_spec(class: &ClassFile, method: &Method) -> anyhow::Result<MethodSpec> {
    let proto = classfile::method_type(&method.descriptor)?;
    let mut modifiers = method_modifiers(method.access);
    if class.access & 0x0200 != 0 && method.code.is_some() && method.access & (0x0008 | 0x0002) == 0
    {
        modifiers.push("default".to_owned());
    }
    Ok(MethodSpec {
        modifiers,
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

/// Unsupported class members degrade to throwing stubs, never empty bodies.
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

/// Decode one selected method; failure is reported instead of guessed Java.
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

/// Whole-class method lowering; unsupported bodies become throwing stubs.
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
    let spec = method_spec(class, method)?;
    let body = method_body(class, method, limits)?;
    let output = render_method_spec(&descriptor_of(class), &simple_name(class), &spec, body);
    Ok(output.source)
}

/// Render all class members; unsupported bodies become explicit throwing stubs.
pub fn render_class(input: &Input, name: &str, limits: Limits) -> anyhow::Result<String> {
    let selected = input.class_by_name(name)?;
    let class = &selected.class;
    let is_interface = class.access & 0x0200 != 0;
    let mut methods = Vec::with_capacity(class.methods.len());
    for method in &class.methods {
        methods.push((
            method_spec(class, method)?,
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

/// String/type pattern or exact method/field selector with optional owner.
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

/// Substring search or exact identity; only unresolved anchors are errors.
#[derive(Clone, Copy, Debug)]
pub struct RefPattern<'a> {
    pub text: &'a str,
    pub exact: bool,
    /// Case mode affects substring searches, never coordinate anchors.
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
    pub fn is_anchor(&self) -> bool {
        match self {
            Self::String(pattern) | Self::Type(pattern) => pattern.exact,
            Self::Method { descriptor, .. } | Self::Field { descriptor, .. } => {
                descriptor.is_some()
            }
        }
    }

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

/// Reference site with caller class/method coordinates.
#[derive(serde::Serialize)]
pub struct RefHit {
    /// Caller method coordinate: Lclass;->name(proto).
    pub method_at: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    pub class: String,
    pub method: String,
    pub method_descriptor: String,
    pub byte_offset: u32,
    pub mnemonic: &'static str,

    pub target: String,
}

/// Scan resolved bytecode operands; return hits in class/method/offset order.
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
                    limits.work.is_none_or(|limit| work <= limit),
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

/// Match a resolved pool operand and return its target description.
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

/// Anchors may be declared members or pool references; no callers still means zero hits.
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
    pub class_bytes: Option<usize>,
    pub expanded_bytes: usize,
    pub entries: usize,
    pub work: Option<usize>,
}

impl Default for Limits {
    fn default() -> Self {
        rxd_core::Budget::default().into()
    }
}

impl From<rxd_core::Budget> for Limits {
    fn from(budget: rxd_core::Budget) -> Self {
        Self {
            input_bytes: budget.input_bytes,
            class_bytes: budget.entry_bytes,
            expanded_bytes: budget.metadata_bytes,
            entries: budget.entries,
            work: budget.work,
        }
    }
}
