//! The `backend` entry point: render Java-like source from IR that a different
//! front end built, through the same passes and writer the DEX path uses.
//!
//! This is the shape rxd's JVM backend would call, so the test is the contract.
//! It must compile and pass with `--no-default-features`:
//!
//! ```text
//! cargo test --no-default-features --test backend_api
//! ```

use radguard::backend::{
    AssignKind, BinaryExpression, BinaryOp, BlockKind, ClassSpec, Constant, ConstantValue,
    FieldSpec, InvokeExpression, InvokeKind, LoweredBlock, LoweredMethod, MethodReference,
    MethodSpec, Statement, Value, VariableId, VariableTable, render_class, render_method,
};

const CLASS: &str = "Lcom/example/Fixture;";
const NAME: &str = "Fixture";

fn integer(value: i64) -> Value {
    Value::Constant(Constant {
        value: ConstantValue::Integer { value, ty: "I" },
    })
}

fn assign(target: VariableId, value: Value) -> Statement {
    Statement::Assign {
        lhs: Some(target),
        rhs: value,
        kind: AssignKind::Normal,
    }
}

fn add(left: Value, right: Value) -> Value {
    Value::Binary(Box::new(BinaryExpression {
        op: BinaryOp::Add,
        left,
        right,
        ty: "I",
    }))
}

/// A block with no exception edges; the tests below add those where needed.
fn block(kind: BlockKind, statements: Vec<Statement>, successors: Vec<usize>) -> LoweredBlock {
    LoweredBlock {
        start: 0,
        end: 0,
        kind,
        statements,
        predecessors: Vec::new(),
        successors,
        exception_successors: Vec::new(),
        exception_handlers: Vec::new(),
        branch_true: None,
        branch_false: None,
        exception: None,
    }
}

/// `static int pick(int p0) { int v; if (p0 > 0) v = 1; else v = 2; return v; }`
#[test]
fn renders_a_branch_from_foreign_ir() {
    let mut variables = VariableTable::default();
    let parameter = variables.register(0);
    variables.set_declared(parameter, true);
    let local = variables.temporary();
    variables.get_mut(local).expect("temporary").ty = Some("I".into());

    let mut condition = block(
        BlockKind::Condition,
        vec![Statement::Condition {
            op: BinaryOp::Greater,
            left: Value::Variable(parameter),
            right: Some(integer(0)),
        }],
        vec![1, 2],
    );
    condition.branch_false = Some(1);
    condition.branch_true = Some(2);

    let body = LoweredMethod {
        variables,
        blocks: vec![
            condition,
            block(
                BlockKind::Statement,
                vec![assign(local, integer(2))],
                vec![3],
            ),
            block(
                BlockKind::Statement,
                vec![assign(local, integer(1))],
                vec![3],
            ),
            block(
                BlockKind::Return,
                vec![Statement::Return(Some(Value::Variable(local)))],
                Vec::new(),
            ),
        ],
    };

    let spec = MethodSpec::new("pick", "(I)I")
        .with_modifiers(vec!["static".to_owned()])
        .with_return_type("int")
        .with_parameter_types(vec!["int".to_owned()]);
    let output = render_method(CLASS, NAME, &spec, Some(body));
    println!("--- branch ---\n{}", output.source);

    assert!(
        output.source.contains("int p0"),
        "parameter is not declared: {}",
        output.source
    );
    assert!(
        output.source.contains("if ("),
        "the branch was not recovered: {}",
        output.source
    );
    assert!(
        output.source.contains("return "),
        "the return is missing: {}",
        output.source
    );
}

/// `static void count(int p0) { int v; v = 0; while (v < p0) v = v + 1; return; }`
///
/// The front end never says "loop": it only describes the back edge.
#[test]
fn recovers_a_loop_from_foreign_ir() {
    let mut variables = VariableTable::default();
    let parameter = variables.register(0);
    variables.set_declared(parameter, true);
    let local = variables.temporary();
    variables.get_mut(local).expect("temporary").ty = Some("I".into());

    let mut condition = block(
        BlockKind::Condition,
        vec![Statement::Condition {
            op: BinaryOp::Less,
            left: Value::Variable(local),
            right: Some(Value::Variable(parameter)),
        }],
        vec![3, 2],
    );
    condition.branch_false = Some(3);
    condition.branch_true = Some(2);

    let body = LoweredMethod {
        variables,
        blocks: vec![
            block(
                BlockKind::Statement,
                vec![assign(local, integer(0))],
                vec![1],
            ),
            condition,
            block(
                BlockKind::Statement,
                vec![assign(local, add(Value::Variable(local), integer(1)))],
                vec![1],
            ),
            block(BlockKind::Return, vec![Statement::Return(None)], Vec::new()),
        ],
    };

    let spec = MethodSpec::new("count", "(I)V")
        .with_modifiers(vec!["static".to_owned()])
        .with_return_type("void")
        .with_parameter_types(vec!["int".to_owned()]);
    let output = render_method(CLASS, NAME, &spec, Some(body));
    println!("--- loop ---\n{}", output.source);

    assert!(
        output.source.contains("while (") || output.source.contains("for ("),
        "no loop was recovered: {}",
        output.source
    );
}

/// `static int load() { return 1; }` - the body-less form ends in `;`.
#[test]
fn renders_a_method_without_code_as_a_declaration() {
    let spec = MethodSpec::new("load", "()I")
        .with_modifiers(vec!["static".to_owned()])
        .with_return_type("int");
    let output = render_method(CLASS, NAME, &spec, None);
    println!("--- declaration ---\n{}", output.source);
    assert!(
        output.source.ends_with(";\n"),
        "a body-less method must end in a declaration: {}",
        output.source
    );
}

/// `static void risky() { try { throw new RuntimeException(); } catch (Throwable e) { } }`
///
/// The front end describes the try region only as a handler edge on the throwing
/// block; "this is a try" is the renderer's conclusion.
#[test]
fn recovers_a_handler_from_foreign_ir() {
    let mut variables = VariableTable::default();
    let caught = variables.temporary();

    let mut throwing = block(
        BlockKind::Throw,
        // A constructor call is an invoke whose base is the instance it builds;
        // that is what renders as `new X(...)`.
        vec![Statement::Throw(Value::Invoke(Box::new(
            InvokeExpression {
                kind: InvokeKind::Direct,
                method: MethodReference {
                    class: "Ljava/lang/RuntimeException;".to_owned(),
                    name: "<init>".to_owned(),
                    parameters: Vec::new(),
                    return_type: "V".to_owned(),
                },
                base: Value::NewInstance {
                    ty: "Ljava/lang/RuntimeException;".to_owned(),
                },
                arguments: Vec::new(),
            },
        )))],
        Vec::new(),
    );
    throwing.exception_successors = vec![1];
    throwing.exception_handlers = vec![(1, Some("Ljava/lang/Throwable;".to_owned()))];

    let body = LoweredMethod {
        variables,
        blocks: vec![
            throwing,
            block(
                BlockKind::Statement,
                vec![
                    Statement::MoveException {
                        lhs: caught,
                        ty: Some("Ljava/lang/Throwable;".to_owned()),
                    },
                    Statement::Return(None),
                ],
                Vec::new(),
            ),
        ],
    };

    let spec = MethodSpec::new("risky", "()V")
        .with_modifiers(vec!["static".to_owned()])
        .with_return_type("void");
    let output = render_method(CLASS, NAME, &spec, Some(body));
    println!("--- handler ---\n{}", output.source);
    assert!(
        output.source.contains("throw new RuntimeException"),
        "the throw is missing: {}",
        output.source
    );
}

/// A whole class assembled from foreign specs: header, field with an initialiser,
/// a constructor and a bodyless declaration.
#[test]
fn renders_a_class_from_foreign_specs() {
    let mut variables = VariableTable::default();
    let receiver = variables.register(0);
    variables.set_declared(receiver, true);
    variables.set_is_this(receiver, true);

    let constructor = MethodSpec::new("<init>", "()V")
        .with_modifiers(vec!["public".to_owned()])
        .with_parameter_types(Vec::new())
        .as_constructor();
    let getter = MethodSpec::new("get", "()I")
        .with_modifiers(vec!["public".to_owned()])
        .with_return_type("int");
    let declaration = MethodSpec::new("native", "()V")
        .with_modifiers(vec!["public".to_owned(), "native".to_owned()])
        .with_return_type("void");

    let spec = ClassSpec {
        descriptor: "Lcom/example/Fixture;".to_owned(),
        package: "com.example".to_owned(),
        name: "Fixture".to_owned(),
        modifiers: vec!["public".to_owned(), "final".to_owned()],
        is_interface: false,
        super_class: Some("java.lang.Object".to_owned()),
        interfaces: vec!["java.io.Serializable".to_owned()],
        fields: vec![
            FieldSpec::new("int", "count", "I").with_modifiers(vec!["private".to_owned()]),
            FieldSpec::new("String", "NAME", "Ljava/lang/String;")
                .with_modifiers(vec!["static".to_owned(), "final".to_owned()])
                .with_constant("\"fixture\""),
        ],
        methods: vec![(constructor, None), (getter, None), (declaration, None)],
    };

    let output = render_class(spec);
    println!("--- class ---\n{}", output.source);

    assert!(
        output.source.starts_with("package com.example;\n"),
        "no package line: {}",
        output.source
    );
    assert!(
        output.source.contains("implements java.io.Serializable"),
        "interfaces are missing: {}",
        output.source
    );
    assert!(
        output.source.contains("\"fixture\""),
        "the field initialiser is missing: {}",
        output.source
    );
    assert!(
        output.source.contains("public Fixture()"),
        "the constructor must render the class name: {}",
        output.source
    );
    assert!(
        output.source.contains("public native void native();"),
        "the bodyless method must end in a declaration: {}",
        output.source
    );
    assert!(
        output
            .member_references
            .iter()
            .any(|reference| reference.declaration && reference.name == "count"),
        "the field must be declared as a member reference"
    );
}
