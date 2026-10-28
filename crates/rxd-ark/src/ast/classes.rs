use super::nodes::Ast;
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::{BinaryOperator as B, LogicalOperator as L, UnaryOperator as U};

fn call<'a>(ast: &Ast<'a>, object: &str, name: &str, args: Vec<Expression<'a>>) -> Expression<'a> {
    ast.call(ast.property(ast.id(object), name), args)
}
fn field<'a>(ast: &Ast<'a>, name: &str) -> Expression<'a> {
    ast.property(ast.id("context"), name)
}
fn context<'a>(ast: &Ast<'a>, extra: Vec<(&str, Expression<'a>)>) -> Expression<'a> {
    let mut entries: Vec<_> = ["env", "hole", "receiver", "newTarget", "locals", "home"]
        .into_iter()
        .filter(|key| !extra.iter().any(|(name, _)| name == key))
        .map(|key| (ast.string(key), field(ast, key)))
        .collect();
    entries.extend(
        extra
            .into_iter()
            .map(|(key, value)| (ast.string(key), value)),
    );
    ast.object(entries)
}
fn define<'a>(
    ast: &Ast<'a>,
    object: Expression<'a>,
    key: Expression<'a>,
    desc: Vec<(&str, Expression<'a>)>,
) -> Statement<'a> {
    ast.expr(call(
        ast,
        "Object",
        "defineProperty",
        vec![
            object,
            key,
            ast.object(
                desc.into_iter()
                    .map(|(key, value)| (ast.string(key), value))
                    .collect(),
            ),
        ],
    ))
}
fn object<'a>(ast: &Ast<'a>, value: &str) -> Expression<'a> {
    Expression::new_logical_expression(
        SPAN,
        ast.binary(
            ast.id(value),
            B::StrictInequality,
            Expression::new_null_literal(SPAN, &ast.build),
        ),
        L::And,
        Expression::new_logical_expression(
            SPAN,
            ast.binary(
                ast.unary(U::Typeof, ast.id(value)),
                B::StrictEquality,
                ast.string("object"),
            ),
            L::Or,
            ast.binary(
                ast.unary(U::Typeof, ast.id(value)),
                B::StrictEquality,
                ast.string("function"),
            ),
            &ast.build,
        ),
        &ast.build,
    )
}

pub(super) fn helpers<'a>(
    ast: &Ast<'a>,
    class_needed: bool,
    method_needed: bool,
    own_needed: bool,
    accessors_needed: bool,
) -> Vec<Statement<'a>> {
    if !(class_needed || method_needed || own_needed || accessors_needed) {
        return vec![];
    }
    let marker = ast.var(
        "__ark_class_objects",
        Some(ast.construct(ast.id("WeakSet"), vec![])),
        true,
    );
    let constructor_marker = ast.var(
        "__ark_class_constructors",
        Some(ast.construct(ast.id("WeakSet"), vec![])),
        true,
    );
    let method = ast.factory(
        "__ark_method",
        vec![
            "factory".into(),
            "length".into(),
            "home".into(),
            "context".into(),
            "name".into(),
        ],
        vec![
            ast.var(
                "fn",
                Some(ast.call(
                    ast.id("factory"),
                    vec![context(ast, vec![("home", ast.id("home"))])],
                )),
                true,
            ),
            define(
                ast,
                ast.id("fn"),
                ast.string("length"),
                vec![("value", ast.id("length"))],
            ),
            ast.if_(
                ast.binary(ast.id("name"), B::StrictInequality, ast.undefined()),
                vec![define(
                    ast,
                    ast.id("fn"),
                    ast.string("name"),
                    vec![("value", ast.id("name"))],
                )],
                None,
            ),
            ast.ret(Some(ast.id("fn"))),
        ],
    );
    let own = ast.factory(
        "__ark_define_own",
        vec!["target".into(), "key".into(), "value".into()],
        vec![define(
            ast,
            ast.id("target"),
            ast.id("key"),
            vec![
                ("value", ast.id("value")),
                ("writable", ast.boolean(true)),
                ("configurable", ast.boolean(true)),
                (
                    "enumerable",
                    ast.unary(
                        U::LogicalNot,
                        call(ast, "__ark_class_objects", "has", vec![ast.id("target")]),
                    ),
                ),
            ],
        )],
    );
    let mut accessor_body = vec![
        ast.var(
            "key",
            Some(ast.member(
                call(
                    ast,
                    "Reflect",
                    "ownKeys",
                    vec![ast.object(vec![(ast.id("prop"), ast.number(0.0))])],
                ),
                ast.number(0.0),
            )),
            true,
        ),
        ast.if_(
            Expression::new_logical_expression(
                SPAN,
                call(
                    ast,
                    "__ark_class_constructors",
                    "has",
                    vec![ast.id("target")],
                ),
                L::And,
                ast.binary(ast.id("key"), B::StrictEquality, ast.string("prototype")),
                &ast.build,
            ),
            vec![ast.throw(ast.construct(
                ast.id("TypeError"),
                vec![ast.string("Invalid static accessor named prototype")],
            ))],
            None,
        ),
        ast.var(
            "desc",
            Some(ast.object(vec![
                (ast.string("configurable"), ast.boolean(true)),
                (
                    ast.string("enumerable"),
                    ast.unary(
                        U::LogicalNot,
                        call(ast, "__ark_class_objects", "has", vec![ast.id("target")]),
                    ),
                ),
            ])),
            true,
        ),
    ];
    for (name, prefix) in [("getter", "get"), ("setter", "set")] {
        let naming = define(
            ast,
            ast.id(name),
            ast.string("name"),
            vec![(
                "value",
                ast.binary(
                    ast.string(&format!("{prefix} ")),
                    B::Addition,
                    ast.call(ast.id("String"), vec![ast.id("key")]),
                ),
            )],
        );
        accessor_body.push(ast.if_(
            ast.binary(ast.id(name), B::StrictInequality, ast.undefined()),
            vec![
                ast.if_(ast.id("setName"), vec![naming], None),
                ast.assign(ast.property(ast.id("desc"), prefix), ast.id(name)),
            ],
            None,
        ));
    }
    accessor_body.push(ast.expr(call(
        ast,
        "Reflect",
        "defineProperty",
        vec![ast.id("target"), ast.id("key"), ast.id("desc")],
    )));
    let accessors = ast.factory(
        "__ark_accessors",
        vec![
            "target".into(),
            "prop".into(),
            "getter".into(),
            "setter".into(),
            "setName".into(),
        ],
        accessor_body,
    );
    [
        (class_needed || own_needed || accessors_needed, marker),
        (class_needed || accessors_needed, constructor_marker),
        (class_needed || method_needed, method),
        (own_needed, own),
        (accessors_needed, accessors),
    ]
    .into_iter()
    .filter_map(|(needed, statement)| needed.then_some(statement))
    .chain(class_needed.then(|| class(ast)))
    .collect()
}

fn class<'a>(ast: &Ast<'a>) -> Statement<'a> {
    let new_target = || Expression::new_new_target(SPAN, &ast.build);
    let constructor_body = vec![
        ast.var(
            "fn",
            Some(ast.call(
                ast.id("factory"),
                vec![context(
                    ast,
                    vec![
                        ("function", ast.id("ctor")),
                        ("home", ast.property(ast.id("ctor"), "prototype")),
                        ("receiver", ast.id("receiver")),
                        ("newTarget", ast.id("newTarget")),
                    ],
                )],
            )),
            true,
        ),
        ast.var(
            "result",
            Some(call(
                ast,
                "Reflect",
                "apply",
                vec![ast.id("fn"), ast.id("receiver"), ast.id("args")],
            )),
            true,
        ),
        ast.if_(
            object(ast, "result"),
            vec![ast.ret(Some(ast.id("result")))],
            None,
        ),
        ast.if_(
            ast.id("derived"),
            vec![ast.throw(ast.construct(
                ast.id("TypeError"),
                vec![ast.string("Derived constructor did not return an object")],
            ))],
            None,
        ),
        ast.ret(Some(ast.id("receiver"))),
    ];
    let make_class = |derived: bool| {
        let invoke = ast.call(
            ast.id("invoke"),
            vec![
                if derived {
                    ast.undefined()
                } else {
                    Expression::new_this_expression(SPAN, &ast.build)
                },
                new_target(),
                ast.id("arguments"),
            ],
        );
        let Expression::FunctionExpression(mut constructor) =
            ast.function("constructor", vec![], vec![ast.ret(Some(invoke))], false)
        else {
            unreachable!()
        };
        constructor.id = None;
        let method = ClassElement::new_method_definition(
            SPAN,
            MethodDefinitionType::MethodDefinition,
            ast.vec([]),
            PropertyKey::from(ast.string("constructor")),
            constructor,
            MethodDefinitionKind::Constructor,
            false,
            false,
            false,
            false,
            None,
            &ast.build,
        );
        Expression::new_class_expression(
            SPAN,
            ClassType::ClassExpression,
            ast.vec([]),
            None,
            None,
            derived.then(|| ClassHeritage::new(ast.id("base"), None, &ast.build)),
            ast.vec([]),
            ClassBody::boxed(SPAN, ast.vec([method]), &ast.build),
            false,
            false,
            &ast.build,
        )
    };
    let mut body = vec![
        ast.var(
            "derived",
            Some(ast.binary(ast.id("base"), B::StrictInequality, field(ast, "hole"))),
            true,
        ),
        ast.var(
            "invoke",
            Some(ast.function(
                "__ark_construct",
                vec!["receiver".into(), "newTarget".into(), "args".into()],
                constructor_body,
                false,
            )),
            true,
        ),
        ast.var(
            "ctor",
            Some(Expression::new_conditional_expression(
                SPAN,
                ast.id("derived"),
                make_class(true),
                make_class(false),
                &ast.build,
            )),
            true,
        ),
        ast.expr(call(
            ast,
            "__ark_class_constructors",
            "add",
            vec![ast.id("ctor")],
        )),
        ast.expr(call(
            ast,
            "__ark_class_objects",
            "add",
            vec![ast.id("ctor")],
        )),
        ast.expr(call(
            ast,
            "__ark_class_objects",
            "add",
            vec![ast.property(ast.id("ctor"), "prototype")],
        )),
        define(
            ast,
            ast.id("ctor"),
            ast.string("name"),
            vec![("value", ast.id("name"))],
        ),
    ];
    let entry = |index| ast.member(ast.id("entry"), ast.number(index));
    let member_body = vec![
        ast.var(
            "home",
            Some(Expression::new_conditional_expression(
                SPAN,
                entry(3.0),
                ast.id("ctor"),
                ast.property(ast.id("ctor"), "prototype"),
                &ast.build,
            )),
            true,
        ),
        ast.var(
            "fn",
            Some(Expression::new_conditional_expression(
                SPAN,
                entry(4.0),
                ast.call(
                    ast.id("__ark_method"),
                    vec![
                        entry(1.0),
                        entry(2.0),
                        ast.id("home"),
                        ast.id("context"),
                        entry(0.0),
                    ],
                ),
                entry(1.0),
                &ast.build,
            )),
            true,
        ),
        ast.if_(
            entry(5.0),
            vec![define(
                ast,
                ast.id("home"),
                entry(0.0),
                vec![
                    ("get", ast.property(ast.id("fn"), "get")),
                    ("set", ast.property(ast.id("fn"), "set")),
                    ("enumerable", ast.boolean(false)),
                    ("configurable", ast.boolean(true)),
                ],
            )],
            Some(vec![define(
                ast,
                ast.id("home"),
                entry(0.0),
                vec![
                    ("value", ast.id("fn")),
                    ("writable", ast.boolean(true)),
                    ("enumerable", ast.boolean(false)),
                    ("configurable", ast.boolean(true)),
                ],
            )]),
        ),
    ];
    let binding = ForStatementLeft::new_variable_declaration(
        SPAN,
        VariableDeclarationKind::Const,
        ast.vec([VariableDeclarator::new(
            SPAN,
            ast.pattern("entry"),
            None,
            None,
            false,
            &ast.build,
        )]),
        false,
        &ast.build,
    );
    body.push(Statement::new_for_of_statement(
        SPAN,
        false,
        binding,
        ast.id("entries"),
        ast.block(member_body),
        &ast.build,
    ));
    body.push(define(
        ast,
        ast.id("ctor"),
        ast.string("length"),
        vec![("value", ast.id("length"))],
    ));
    body.push(ast.ret(Some(ast.id("ctor"))));
    ast.factory(
        "__ark_class",
        vec![
            "factory".into(),
            "base".into(),
            "length".into(),
            "entries".into(),
            "context".into(),
            "name".into(),
        ],
        body,
    )
}
