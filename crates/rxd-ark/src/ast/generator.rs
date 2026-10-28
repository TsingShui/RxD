use super::nodes::Ast;
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::{BinaryOperator as B, LogicalOperator as L};

pub(super) fn helpers<'a>(ast: &Ast<'a>) -> Vec<Statement<'a>> {
    let registry = ast.var(
        "__ark_generator_state",
        Some(ast.construct(ast.id("WeakMap"), vec![])),
        true,
    );
    let suspend = ast.factory(
        "__ark_suspend",
        vec!["current".into(), "target".into(), "value".into()],
        vec![
            ast.if_(
                ast.binary(ast.id("current"), B::StrictInequality, ast.id("target")),
                vec![ast.throw(ast.construct(
                    ast.id("TypeError"),
                    vec![ast.string("Generator suspension targets another frame")],
                ))],
                None,
            ),
            ast.ret(Some(ast.id("value"))),
        ],
    );
    let result = |value| {
        ast.object(vec![
            (ast.string("value"), value),
            (ast.string("done"), ast.boolean(true)),
        ])
    };
    let mut run_body =
        vec![
            ast.if_(
                ast.binary(ast.id("receiver"), B::StrictInequality, ast.id("target")),
                vec![ast.throw(ast.construct(
                    ast.id("TypeError"),
                    vec![ast.string("Invalid generator receiver")],
                ))],
                None,
            ),
            ast.if_(
                ast.id("running"),
                vec![ast.throw(
                    ast.construct(ast.id("TypeError"), vec![ast.string("State is executing")]),
                )],
                None,
            ),
            ast.if_(
                Expression::new_logical_expression(
                    SPAN,
                    ast.id("started"),
                    L::And,
                    ast.binary(ast.id("mode"), B::StrictInequality, ast.number(2.0)),
                    &ast.build,
                ),
                vec![ast.assign(ast.id("closed"), ast.boolean(true))],
                None,
            ),
            ast.if_(
                ast.id("closed"),
                vec![
                    ast.if_(
                        ast.binary(ast.id("mode"), B::StrictEquality, ast.number(1.0)),
                        vec![ast.throw(ast.id("value"))],
                        None,
                    ),
                    ast.ret(Some(result(Expression::new_conditional_expression(
                        SPAN,
                        ast.binary(ast.id("mode"), B::StrictEquality, ast.number(0.0)),
                        ast.id("value"),
                        ast.undefined(),
                        &ast.build,
                    )))),
                ],
                None,
            ),
            ast.assign(ast.id("started"), ast.boolean(false)),
            ast.assign(ast.property(ast.id("state"), "mode"), ast.id("mode")),
            ast.assign(ast.property(ast.id("state"), "value"), ast.id("value")),
            ast.assign(ast.id("running"), ast.boolean(true)),
            ast.var("step", None, false),
            ast.try_catch(
                vec![ast.assign(
                    ast.id("step"),
                    ast.call(ast.property(ast.id("engine"), "next"), vec![]),
                )],
                "error",
                vec![
                    ast.assign(ast.id("running"), ast.boolean(false)),
                    ast.assign(ast.id("closed"), ast.boolean(true)),
                    ast.throw(ast.id("error")),
                ],
            ),
            ast.assign(ast.id("running"), ast.boolean(false)),
            ast.assign(ast.id("closed"), ast.property(ast.id("step"), "done")),
        ];
    run_body.push(ast.ret(Some(Expression::new_conditional_expression(
        SPAN,
        ast.id("closed"),
        ast.id("step"),
        ast.property(ast.id("step"), "value"),
        &ast.build,
    ))));
    let mut body = vec![
        ast.var(
            "state",
            Some(ast.object(vec![
                (ast.string("mode"), ast.number(2.0)),
                (ast.string("value"), ast.undefined()),
            ])),
            true,
        ),
        ast.expr(ast.call(
            ast.property(ast.id("__ark_generator_state"), "set"),
            vec![ast.id("target"), ast.id("state")],
        )),
        ast.var("started", Some(ast.boolean(true)), false),
        ast.var("closed", Some(ast.boolean(false)), false),
        ast.var("running", Some(ast.boolean(false)), false),
        ast.var(
            "run",
            Some(ast.function(
                "__ark_resume",
                vec!["receiver".into(), "mode".into(), "value".into()],
                run_body,
                false,
            )),
            true,
        ),
    ];
    for (name, mode) in [("next", 2.0), ("throw", 1.0), ("return", 0.0)] {
        let method = ast.function(
            &format!("__ark_{name}"),
            vec!["value".into()],
            vec![ast.ret(Some(ast.call(
                ast.id("run"),
                vec![
                    Expression::new_this_expression(SPAN, &ast.build),
                    ast.number(mode),
                    ast.id("value"),
                ],
            )))],
            false,
        );
        body.push(ast.expr(ast.call(
            ast.property(ast.id("Object"), "defineProperty"),
            vec![
                ast.id("target"),
                ast.string(name),
                ast.object(vec![
                    (ast.string("value"), method),
                    (ast.string("writable"), ast.boolean(true)),
                    (ast.string("configurable"), ast.boolean(true)),
                ]),
            ],
        )));
    }
    let iterable = ast.function(
        "iterator",
        vec![],
        vec![ast.ret(Some(Expression::new_this_expression(SPAN, &ast.build)))],
        false,
    );
    body.push(ast.expr(ast.call(
        ast.property(ast.id("Object"), "defineProperty"),
        vec![
            ast.id("target"),
            ast.property(ast.id("Symbol"), "iterator"),
            ast.object(vec![
                (ast.string("value"), iterable),
                (ast.string("configurable"), ast.boolean(true)),
            ]),
        ],
    )));
    // Prime only the creation/prefix up to the initial suspension, matching Ark's eager frame setup.
    body.push(ast.var(
        "initial",
        Some(ast.call(ast.property(ast.id("engine"), "next"), vec![])),
        true,
    ));
    body.push(ast.if_(
        ast.property(ast.id("initial"), "done"),
        vec![ast.assign(ast.id("closed"), ast.boolean(true))],
        None,
    ));
    body.push(ast.ret(Some(ast.id("target"))));
    vec![
        registry,
        suspend,
        ast.factory("__ark_drive", vec!["engine".into(), "target".into()], body),
    ]
}
