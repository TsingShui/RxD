use super::nodes::Ast;
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::{BinaryOperator as B, LogicalOperator as L};

pub(super) fn alias<'a>(ast: &Ast<'a>) -> Statement<'a> {
    let mut body = vec![
        ast.var("target", Some(ast.object(vec![])), true),
        ast.expr(ast.call(
            ast.property(ast.id("__ark_generator_state"), "set"),
            vec![
                ast.id("target"),
                ast.call(
                    ast.property(ast.id("__ark_generator_state"), "get"),
                    vec![ast.id("current")],
                ),
            ],
        )),
    ];
    for name in ["next", "throw", "return"] {
        let function = ast.function(
            &format!("__ark_alias_{name}"),
            vec!["value".into()],
            vec![ast.ret(Some(ast.call(
                ast.property(ast.id("Reflect"), "apply"),
                vec![
                    ast.property(ast.id("current"), name),
                    ast.id("current"),
                    ast.id("arguments"),
                ],
            )))],
            false,
        );
        body.push(ast.assign(ast.property(ast.id("target"), name), function));
    }
    for name in ["iterator", "asyncIterator"] {
        let key = || ast.property(ast.id("Symbol"), name);
        body.push(ast.if_(
            ast.member(ast.id("current"), key()),
            vec![ast.assign(
                ast.member(ast.id("target"), key()),
                ast.function(
                    "__ark_alias_self",
                    vec![],
                    vec![ast.ret(Some(Expression::new_this_expression(SPAN, &ast.build)))],
                    false,
                ),
            )],
            None,
        ));
    }
    body.push(ast.ret(Some(ast.id("target"))));
    ast.factory("__ark_generator_alias", vec!["current".into()], body)
}

pub(super) fn helpers<'a>(ast: &Ast<'a>, asynchronous: bool) -> Vec<Statement<'a>> {
    let next = || {
        let call = ast.call(ast.property(ast.id("engine"), "next"), vec![]);
        if asynchronous {
            Expression::new_await_expression(SPAN, call, &ast.build)
        } else {
            call
        }
    };
    let mut advance = vec![ast.assign(ast.id("step"), next())];
    if asynchronous {
        let internal = Expression::new_logical_expression(
            SPAN,
            ast.unary(
                oxc_syntax::operator::UnaryOperator::LogicalNot,
                ast.property(ast.id("step"), "done"),
            ),
            L::And,
            Expression::new_logical_expression(
                SPAN,
                ast.property(ast.id("step"), "value"),
                L::And,
                ast.property(ast.property(ast.id("step"), "value"), "__ark_internal"),
                &ast.build,
            ),
            &ast.build,
        );
        advance.push(Statement::new_while_statement(
            SPAN,
            internal,
            ast.block(vec![ast.assign(ast.id("step"), next())]),
            &ast.build,
        ));
    }
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
                Expression::new_logical_expression(
                    SPAN,
                    ast.binary(ast.id("current"), B::StrictInequality, ast.id("target")),
                    L::And,
                    ast.binary(
                        ast.call(
                            ast.property(ast.id("__ark_generator_state"), "get"),
                            vec![ast.id("current")],
                        ),
                        B::StrictInequality,
                        ast.call(
                            ast.property(ast.id("__ark_generator_state"), "get"),
                            vec![ast.id("target")],
                        ),
                    ),
                    &ast.build,
                ),
                vec![ast.throw(ast.construct(
                    ast.id("TypeError"),
                    vec![ast.string("Generator suspension targets another frame")],
                ))],
                None,
            ),
            ast.assign(
                ast.property(
                    ast.call(
                        ast.property(ast.id("__ark_generator_state"), "get"),
                        vec![ast.id("current")],
                    ),
                    "suspendedTarget",
                ),
                ast.id("target"),
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
                        if asynchronous {
                            Expression::new_await_expression(SPAN, ast.id("value"), &ast.build)
                        } else {
                            ast.id("value")
                        },
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
                advance,
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
    if asynchronous {
        run_body.push(ast.if_(
            ast.property(ast.id("state"), "failed"),
            vec![
                ast.assign(ast.property(ast.id("state"), "failed"), ast.boolean(false)),
                ast.throw(ast.property(ast.id("state"), "error")),
            ],
            None,
        ));
        run_body.push(ast.if_(
            Expression::new_logical_expression(
                SPAN,
                ast.unary(
                    oxc_syntax::operator::UnaryOperator::LogicalNot,
                    ast.property(ast.id("step"), "done"),
                ),
                L::And,
                ast.property(ast.property(ast.id("step"), "value"), "done"),
                &ast.build,
            ),
            vec![ast.assign(ast.id("closed"), ast.boolean(true))],
            None,
        ));
    }
    run_body.push(ast.ret(Some(Expression::new_conditional_expression(
        SPAN,
        ast.property(ast.id("step"), "done"),
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
                asynchronous,
            )),
            true,
        ),
    ];
    for (name, mode) in [("next", 2.0), ("throw", 1.0), ("return", 0.0)] {
        let run = ast.call(
            ast.id("run"),
            vec![
                if asynchronous {
                    ast.id("receiver")
                } else {
                    Expression::new_this_expression(SPAN, &ast.build)
                },
                ast.number(mode),
                ast.id("value"),
            ],
        );
        let method_body = if asynchronous {
            let task = ast.call(
                ast.property(ast.id("pending"), "then"),
                vec![ast.function("__ark_request", vec![], vec![ast.ret(Some(run))], false)],
            );
            let settle = || {
                ast.function(
                    "__ark_settled",
                    vec![],
                    vec![ast.ret(Some(ast.undefined()))],
                    false,
                )
            };
            vec![
                ast.var(
                    "receiver",
                    Some(Expression::new_this_expression(SPAN, &ast.build)),
                    true,
                ),
                ast.var("task", Some(task), true),
                ast.assign(
                    ast.id("pending"),
                    ast.call(
                        ast.property(ast.id("task"), "then"),
                        vec![settle(), settle()],
                    ),
                ),
                ast.ret(Some(ast.id("task"))),
            ]
        } else {
            vec![ast.ret(Some(run))]
        };
        let method = ast.function(
            &format!("__ark_{name}"),
            vec!["value".into()],
            method_body,
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
            ast.property(
                ast.id("Symbol"),
                if asynchronous {
                    "asyncIterator"
                } else {
                    "iterator"
                },
            ),
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
    let close = ast.if_(
        ast.property(ast.id("initial"), "done"),
        vec![ast.assign(ast.id("closed"), ast.boolean(true))],
        None,
    );
    if asynchronous {
        body.push(ast.var(
            "pending",
            Some(ast.call(
                ast.property(ast.id("initial"), "then"),
                vec![ast.function("__ark_started", vec!["initial".into()], vec![close], false)],
            )),
            false,
        ));
    } else {
        body.push(close);
    }
    body.push(ast.ret(Some(Expression::new_logical_expression(
        SPAN,
        ast.property(ast.id("state"), "suspendedTarget"),
        L::Or,
        ast.id("target"),
        &ast.build,
    ))));
    let drive = ast.factory(
        if asynchronous {
            "__ark_async_drive"
        } else {
            "__ark_drive"
        },
        vec!["engine".into(), "target".into()],
        body,
    );
    if asynchronous {
        vec![drive]
    } else {
        vec![registry, suspend, drive]
    }
}
