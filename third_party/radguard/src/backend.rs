//! Java-like rendering for IR built by another front end.
//!
//! The DEX path ([`crate::Decompiler`]) is the reference front end: it lowers
//! Dalvik code into the IR re-exported here and renders it. A different bytecode
//! family (rxd's JVM backend) can build the same IR itself and get the same
//! renderer back - the same passes, the same output shape - which is why these
//! types are public here while the passes behind them stay private.
//!
//! # IR contract
//!
//! * `LoweredMethod::blocks` order defines block identity; **block 0 is the entry**.
//! * Every edge - `successors`, `predecessors`, `exception_successors`, and the
//!   first element of each `exception_handlers` pair - is an **index into that
//!   vector**, not an address.
//! * `predecessors` may be left empty: it is recomputed from `successors`.
//! * `BlockKind` only has to tell `Statement`, `Return`, `Throw`, `Condition` and
//!   `Switch` apart. Loops, short circuits and try/catch regions are recovered by
//!   the passes the renderer runs, so a front end never classifies those itself.
//! * For a `Condition` block, `successors` is `[fallthrough, branch target]` and
//!   `branch_false`/`branch_true` point at those two indices, matching the order
//!   `Statement::Condition` describes.
//! * Variables are plain registers and temporaries. SSA versions, dead-code
//!   elimination, declaration placement and expression inlining all happen inside
//!   the renderer, so a front end emits straight-line statements per block.
//! * A constructor call is an [`Value::Invoke`] whose `base` is the
//!   [`Value::NewInstance`] it builds and whose method is `<init>`; that is what
//!   renders as `new X(...)`.
//! * Only blocks reachable from block 0 are kept.

pub use crate::ir::opcode::{BlockKind, LoweredBlock, LoweredMethod};
pub use crate::ir::{
    ArrayLoadExpression, ArrayStoreStatement, AssignKind, BinaryExpression, BinaryOp,
    CheckCastExpression, Constant, ConstantValue, FieldReference, FilledArrayExpression,
    InstanceFieldExpression, InstancePutStatement, InvokeExpression, InvokeKind, MethodReference,
    NewArrayExpression, Statement, UnaryExpression, UnaryOp, Value, VariableId, VariableTable,
};
pub use crate::types::descriptor_to_java;
pub use crate::writer::{MemberKind, MemberReference, SourceOutput};

use crate::render::{self, ClassHeader, FieldHeader, MethodHeader};

/// A field's declared shape, with the type already rendered as Java.
pub struct FieldSpec {
    /// Rendered Java modifiers.
    pub modifiers: Vec<String>,
    /// Rendered Java type.
    pub ty: String,
    pub name: String,
    /// Declared descriptor, used for the member reference.
    pub descriptor: String,
    /// Rendered initialiser, absent when the field has none.
    pub constant: Option<String>,
}

impl FieldSpec {
    pub fn new(
        ty: impl Into<String>,
        name: impl Into<String>,
        descriptor: impl Into<String>,
    ) -> Self {
        Self {
            modifiers: Vec::new(),
            ty: ty.into(),
            name: name.into(),
            descriptor: descriptor.into(),
            constant: None,
        }
    }

    pub fn with_modifiers(mut self, modifiers: Vec<String>) -> Self {
        self.modifiers = modifiers;
        self
    }

    pub fn with_constant(mut self, constant: impl Into<String>) -> Self {
        self.constant = Some(constant.into());
        self
    }
}

/// A whole class: header, fields and methods, with bodies already lowered.
pub struct ClassSpec {
    /// The class descriptor, e.g. `Lcom/example/Fixture;`. Every member reference
    /// this renders is attributed to it.
    pub descriptor: String,
    /// Dotted package; empty means no package line.
    pub package: String,
    /// Simple name, used for the declaration and for constructor names.
    pub name: String,
    /// Rendered Java modifiers.
    pub modifiers: Vec<String>,
    pub is_interface: bool,
    /// Dotted superclass; omit `java.lang.Object` here, as the DEX path does.
    pub super_class: Option<String>,
    /// Dotted interfaces.
    pub interfaces: Vec<String>,
    pub fields: Vec<FieldSpec>,
    /// Methods in declaration order. `None` renders a bodyless declaration.
    pub methods: Vec<(MethodSpec, Option<LoweredMethod>)>,
}

impl ClassSpec {
    pub fn new(
        descriptor: impl Into<String>,
        package: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            descriptor: descriptor.into(),
            package: package.into(),
            name: name.into(),
            modifiers: Vec::new(),
            is_interface: false,
            super_class: None,
            interfaces: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
        }
    }
}

/// Renders a whole class, through the same passes and writer the DEX path uses.
pub fn render_class(spec: ClassSpec) -> SourceOutput {
    let mut output = SourceOutput::default();
    let mut modifiers = spec.modifiers.clone();
    if spec.is_interface && !modifiers.iter().any(|modifier| modifier == "interface") {
        modifiers.push("interface".to_owned());
    }
    let (super_class, interfaces) = if spec.is_interface {
        (
            (!spec.interfaces.is_empty()).then(|| spec.interfaces.join(", ")),
            Vec::new(),
        )
    } else {
        (spec.super_class.clone(), spec.interfaces.clone())
    };
    render::write_class_header(
        &mut output,
        &ClassHeader {
            package: spec.package.clone(),
            modifiers,
            is_interface: spec.is_interface,
            name: spec.name.clone(),
            super_class,
            interfaces,
        },
    );
    for field in &spec.fields {
        render::write_field(
            &mut output,
            &spec.descriptor,
            &FieldHeader {
                modifiers: field.modifiers.clone(),
                ty: field.ty.clone(),
                name: field.name.clone(),
                descriptor: field.descriptor.clone(),
                constant: field.constant.clone(),
            },
        );
    }
    for (method, body) in spec.methods {
        let header = MethodHeader {
            modifiers: method.modifiers.clone(),
            is_constructor: method.is_constructor,
            return_type: method.return_type.clone(),
            name: method.name.clone(),
            descriptor: method.descriptor.clone(),
            parameter_types: method.parameter_types.clone(),
            annotations: Vec::new(),
            style: crate::render::SourceStyle::FOREIGN,
            indent: true,
        };
        output = render::write_method(output, &spec.descriptor, &spec.name, &header, body);
    }
    output.source.push_str("}\n");
    output
}

/// A method's declared shape, with Java-rendered types.
pub struct MethodSpec {
    /// Rendered Java modifiers, e.g. `["public", "static"]`.
    pub modifiers: Vec<String>,
    /// A constructor renders the class name and omits the return type.
    pub is_constructor: bool,
    /// Rendered Java return type; ignored for a constructor.
    pub return_type: String,
    /// Declared name, used for the member reference.
    pub name: String,
    /// Declared descriptor, used for the member reference.
    pub descriptor: String,
    /// Rendered Java parameter types, in declaration order.
    pub parameter_types: Vec<String>,
}

impl MethodSpec {
    /// A method with no modifiers and no parameters.
    pub fn new(name: impl Into<String>, descriptor: impl Into<String>) -> Self {
        Self {
            modifiers: Vec::new(),
            is_constructor: false,
            return_type: String::new(),
            name: name.into(),
            descriptor: descriptor.into(),
            parameter_types: Vec::new(),
        }
    }

    pub fn with_modifiers(mut self, modifiers: Vec<String>) -> Self {
        self.modifiers = modifiers;
        self
    }

    pub fn with_return_type(mut self, return_type: impl Into<String>) -> Self {
        self.return_type = return_type.into();
        self
    }

    pub fn with_parameter_types(mut self, parameter_types: Vec<String>) -> Self {
        self.parameter_types = parameter_types;
        self
    }

    pub fn as_constructor(mut self) -> Self {
        self.is_constructor = true;
        self
    }
}

/// Renders one method, body included, the way the DEX front end renders it.
///
/// `body` is `None` for a method without code, which renders as a declaration
/// ending in `;`. The parameters a body declares are matched to
/// [`MethodSpec::parameter_types`] in declaration order, so a front end registers
/// them in that order and marks them with
/// [`VariableTable::set_declared`] (and `set_is_this` for the receiver).
pub fn render_method(
    class_descriptor: &str,
    class_name: &str,
    spec: &MethodSpec,
    body: Option<LoweredMethod>,
) -> SourceOutput {
    let header = MethodHeader {
        modifiers: spec.modifiers.clone(),
        is_constructor: spec.is_constructor,
        return_type: spec.return_type.clone(),
        name: spec.name.clone(),
        descriptor: spec.descriptor.clone(),
        parameter_types: spec.parameter_types.clone(),
        annotations: Vec::new(),
        style: crate::render::SourceStyle::FOREIGN,
        indent: false,
    };
    render::write_method(
        SourceOutput::default(),
        class_descriptor,
        class_name,
        &header,
        body,
    )
}
