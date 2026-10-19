//! Method rendering shared by the DEX front end and the [`crate::backend`] entry.
//!
//! The DEX path fills [`MethodHeader`] from `DexFile` metadata; `backend` fills it
//! from a caller-supplied spec. Everything from the declared header on is one
//! implementation, so a foreign front end renders exactly like the DEX one.

use crate::graph::Graph;
use crate::ir::opcode::LoweredMethod;
use crate::writer::{MemberKind, MemberReference, MethodWriter, SourceOutput};

/// A class's declared shape, with package and types already rendered as Java.
pub(crate) struct ClassHeader {
    /// Dotted package, empty when the class has none (then no package line).
    pub package: String,
    /// Rendered Java modifiers.
    pub modifiers: Vec<String>,
    pub is_interface: bool,
    /// Simple name, without the package.
    pub name: String,
    /// Dotted superclass, already omitted when it is `java.lang.Object`.
    pub super_class: Option<String>,
    /// Dotted interfaces.
    pub interfaces: Vec<String>,
}

/// A field's declared shape, with the type already rendered as Java.
pub(crate) struct FieldHeader {
    pub modifiers: Vec<String>,
    pub ty: String,
    pub name: String,
    /// Declared descriptor, used for the member reference.
    pub descriptor: String,
    /// Rendered initialiser, absent when the field has none.
    pub constant: Option<String>,
}

/// Writes everything up to and including the class body's opening brace.
///
/// The modifier list is followed by a space even when it is empty. That is not a
/// slip: it is what the frozen DAD parity gate pins for package-private classes,
/// and both front ends render through this function so they agree.
pub(crate) fn write_class_header(output: &mut SourceOutput, header: &ClassHeader) {
    if !header.package.is_empty() {
        output.source.push_str("package ");
        output.source.push_str(&header.package);
        output.source.push_str(";\n");
    }
    output.source.push_str(&header.modifiers.join(" "));
    output.source.push(' ');
    if !header.is_interface {
        output.source.push_str("class ");
    }
    output.source.push_str(&header.name);
    if let Some(super_class) = &header.super_class {
        output.source.push_str(" extends ");
        output.source.push_str(super_class);
    }
    if !header.interfaces.is_empty() {
        output.source.push_str(" implements ");
        output.source.push_str(&header.interfaces.join(", "));
    }
    output.source.push_str(" {\n");
}

/// Writes one field declaration.
pub(crate) fn write_field(output: &mut SourceOutput, class: &str, field: &FieldHeader) {
    output.source.push_str("    ");
    if !field.modifiers.is_empty() {
        output.source.push_str(&field.modifiers.join(" "));
        output.source.push(' ');
    }
    output.source.push_str(&field.ty);
    output.source.push(' ');
    let start = output.source.len();
    output.source.push_str(&field.name);
    output.member_references.push(MemberReference {
        start,
        end: output.source.len(),
        kind: MemberKind::Field,
        class: class.to_owned(),
        name: field.name.clone(),
        descriptor: field.descriptor.clone(),
        declaration: true,
    });
    if let Some(constant) = &field.constant {
        output.source.push_str(" = ");
        output.source.push_str(constant);
    }
    output.source.push_str(";\n");
}

/// Rendering switches a front end chooses.
///
/// The DEX path is frozen against Androguard's DAD output, so it takes
/// [`SourceStyle::DAD`]. A foreign front end (the JVM backend) asks for the
/// spellings DAD has no opinion on, because its front end knows facts DEX
/// encodings do not carry.
#[derive(Clone, Copy)]
pub(crate) struct SourceStyle {
    /// Constants in a `boolean` position render as `false`/`true`.
    ///
    /// The JVM stack only has `int`, so `boolean` arrives as 0/1 and the declared
    /// type is the only thing that can tell the renderer; DEX constants already
    /// carry a boolean type, and the frozen DAD parity gate pins that rendering.
    pub boolean_literals: bool,
}

impl SourceStyle {
    /// What the frozen DAD parity gate pins.
    pub(crate) const DAD: Self = Self {
        boolean_literals: false,
    };

    /// What a foreign front end wants.
    pub(crate) const FOREIGN: Self = Self {
        boolean_literals: true,
    };
}

/// A method's declared shape, with every type already rendered as Java.
pub(crate) struct MethodHeader {
    /// Rendered Java modifiers, without the constructor marker.
    pub modifiers: Vec<String>,
    pub is_constructor: bool,
    pub return_type: String,
    /// Declared name; a constructor displays the class name instead.
    pub name: String,
    pub descriptor: String,
    /// Rendered Java parameter types, in declaration order.
    pub parameter_types: Vec<String>,
    /// Rendered annotations, emitted before the signature.
    pub annotations: Vec<String>,
    /// The rendering switches this front end asks for.
    pub style: SourceStyle,
    /// Whether the method is a class member (indented, after a blank line) or a
    /// standalone rendering a caller prints on its own.
    pub indent: bool,
}

/// Writes one method onto `output`, body included.
///
/// `body` is `None` for a method without code, which renders as a declaration
/// ending in `;`. The IR contract is documented on [`crate::backend`].
pub(crate) fn write_method(
    mut output: SourceOutput,
    class_descriptor: &str,
    class_name: &str,
    header: &MethodHeader,
    body: Option<LoweredMethod>,
) -> SourceOutput {
    let separator = if header.indent { "\n    " } else { "" };
    for annotation in &header.annotations {
        output.source.push_str(separator);
        output.source.push_str(annotation);
    }
    output.source.push_str(separator);
    if !header.modifiers.is_empty() {
        output.source.push_str(&header.modifiers.join(" "));
        output.source.push(' ');
    }
    if !header.is_constructor {
        output.source.push_str(&header.return_type);
        output.source.push(' ');
    }
    let displayed_name = if header.is_constructor {
        class_name
    } else {
        &header.name
    };
    let name_start = output.source.len();
    output.source.push_str(displayed_name);
    output.member_references.push(MemberReference {
        start: name_start,
        end: output.source.len(),
        kind: MemberKind::Method,
        class: class_descriptor.to_owned(),
        name: header.name.clone(),
        descriptor: header.descriptor.clone(),
        declaration: true,
    });

    let mut graph = body.map(Graph::from_lowered);
    let parameter_registers = graph
        .as_ref()
        .map(|graph| {
            let mut parameters: Vec<_> = graph
                .variables
                .parameter_ids()
                .into_iter()
                .filter(|variable| !graph.variables.is_this(*variable))
                .collect();
            parameters.sort();
            parameters
        })
        .unwrap_or_default();
    output.source.push('(');
    for (index, (parameter_type, parameter)) in header
        .parameter_types
        .iter()
        .zip(parameter_registers.iter())
        .enumerate()
    {
        if index != 0 {
            output.source.push_str(", ");
        }
        output.source.push_str(parameter_type);
        output.source.push(' ');
        let named = graph
            .as_ref()
            .is_some_and(|graph| graph.variables.is_named(*parameter));
        if named {
            let name = graph
                .as_ref()
                .and_then(|graph| graph.variables.name(*parameter))
                .unwrap_or("unknown");
            output.source.push_str(name);
            continue;
        }
        output.source.push('p');
        let name = graph
            .as_ref()
            .and_then(|graph| graph.variables.name(*parameter))
            .unwrap_or("unknown");
        output.source.push_str(name);
    }
    output.source.push(')');

    let Some(graph) = graph.as_mut() else {
        output.source.push_str(";\n");
        return output;
    };
    crate::dataflow::optimize_for_decompilation(graph);
    graph.split_condition_nodes();
    graph.simplify();
    graph.compute_rpo();
    crate::control_flow::identify_structures(graph);
    MethodWriter::new_moving_conditions(
        graph,
        class_descriptor,
        header.is_constructor,
        output,
        header.style.boolean_literals,
        header.indent,
    )
    .write_body()
}
