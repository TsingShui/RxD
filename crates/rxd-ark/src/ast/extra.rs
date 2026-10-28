use super::nodes::Ast;
use crate::ir::Intrinsic as I;
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::{BinaryOperator as B, LogicalOperator as L, UnaryOperator as U};
use std::collections::BTreeSet;

pub(super) fn call<'a>(
    a: &Ast<'a>,
    object: &str,
    name: &str,
    args: Vec<Expression<'a>>,
) -> Expression<'a> {
    a.call(a.property(a.id(object), name), args)
}
pub(super) fn object<'a>(a: &Ast<'a>, name: &str) -> Expression<'a> {
    Expression::new_logical_expression(
        SPAN,
        a.binary(
            a.id(name),
            B::StrictInequality,
            Expression::new_null_literal(SPAN, &a.build),
        ),
        L::And,
        Expression::new_logical_expression(
            SPAN,
            a.binary(
                a.unary(U::Typeof, a.id(name)),
                B::StrictEquality,
                a.string("object"),
            ),
            L::Or,
            a.binary(
                a.unary(U::Typeof, a.id(name)),
                B::StrictEquality,
                a.string("function"),
            ),
            &a.build,
        ),
        &a.build,
    )
}
pub(super) fn error<'a>(a: &Ast<'a>, kind: &str, message: &str) -> Statement<'a> {
    a.throw(a.construct(a.id(kind), vec![a.string(message)]))
}
pub(super) fn loop_of<'a>(
    a: &Ast<'a>,
    key: &str,
    value: Expression<'a>,
    body: Vec<Statement<'a>>,
) -> Statement<'a> {
    let binding = ForStatementLeft::new_variable_declaration(
        SPAN,
        VariableDeclarationKind::Const,
        a.vec([VariableDeclarator::new(
            SPAN,
            a.pattern(key),
            None,
            None,
            false,
            &a.build,
        )]),
        false,
        &a.build,
    );
    Statement::new_for_of_statement(SPAN, false, binding, value, a.block(body), &a.build)
}
fn helper<'a>(a: &Ast<'a>, kind: I, params: &[&str], body: Vec<Statement<'a>>) -> Statement<'a> {
    a.factory(
        &format!("__ark_{}", kind.name()),
        params.iter().map(|s| (*s).into()).collect(),
        body,
    )
}
pub(super) fn helpers<'a>(a: &Ast<'a>, needed: &BTreeSet<I>) -> Vec<Statement<'a>> {
    let mut out = vec![];
    if needed.contains(&I::Template) {
        out.push(a.var(
            "__ark_templates",
            Some(a.construct(a.id("WeakMap"), vec![])),
            true,
        ));
    }
    if needed.iter().any(|i| {
        matches!(
            i,
            I::GlobalGet | I::GlobalSet | I::GlobalDeclare | I::PatchGet | I::PatchSet
        )
    }) {
        out.push(a.var(
            "__ark_globals",
            Some(a.construct(a.id("Map"), vec![])),
            true,
        ));
        out.push(a.var("__ark_patches", Some(a.array(vec![])), true));
    }
    for kind in needed {
        let statement = match *kind {
            I::AsyncEnter
            | I::AsyncAwait
            | I::AsyncResolve
            | I::AsyncReject
            | I::AsyncGenReject
            | I::AsyncPause
            | I::GeneratorState => continue,
            I::Accessor
            | I::BigInt
            | I::Arguments
            | I::Symbol
            | I::ConstructSpread
            | I::SendableClass
            | I::SendableEnv
            | I::SendableGet
            | I::SendableSet
            | I::SendableLoadClass
            | I::SendableImport
            | I::LazyImport
            | I::ConcurrentResult => continue,
            I::PrivateGet | I::PrivateSet | I::PrivateDefine | I::PrivateHas | I::PrivateKeys => {
                continue;
            }
            I::ObjectLiteral => {
                let field = |index| a.member(a.id("entry"), a.number(index));
                let normal = a.object(vec![
                    (a.string("value"), field(1.0)),
                    (a.string("writable"), a.boolean(true)),
                    (a.string("configurable"), a.boolean(true)),
                    (a.string("enumerable"), a.boolean(true)),
                ]);
                let accessor = a.object(vec![
                    (a.string("get"), a.property(field(1.0), "get")),
                    (a.string("set"), a.property(field(1.0), "set")),
                    (a.string("configurable"), a.boolean(true)),
                    (a.string("enumerable"), a.boolean(true)),
                ]);
                helper(
                    a,
                    *kind,
                    &["entries"],
                    vec![
                        a.var("result", Some(a.object(vec![])), true),
                        loop_of(
                            a,
                            "entry",
                            a.id("entries"),
                            vec![a.expr(call(
                                a,
                                "Object",
                                "defineProperty",
                                vec![
                                    a.id("result"),
                                    field(0.0),
                                    Expression::new_conditional_expression(
                                        SPAN,
                                        field(2.0),
                                        accessor,
                                        normal,
                                        &a.build,
                                    ),
                                ],
                            ))],
                        ),
                        a.ret(Some(a.id("result"))),
                    ],
                )
            }
            I::RegExp => {
                let flags = a.array(
                    [
                        (1.0, "g"),
                        (2.0, "i"),
                        (4.0, "m"),
                        (8.0, "s"),
                        (16.0, "u"),
                        (32.0, "y"),
                        (64.0, "d"),
                    ]
                    .into_iter()
                    .map(|(bit, name)| a.array(vec![a.number(bit), a.string(name)]))
                    .collect(),
                );
                helper(
                    a,
                    *kind,
                    &["pattern", "flags"],
                    vec![
                        a.var("text", Some(a.string("")), false),
                        a.if_(
                            a.binary(a.id("flags"), B::BitwiseAnd, a.number(128.0)),
                            vec![error(
                                a,
                                "RangeError",
                                "Unknown Ark regular-expression flag",
                            )],
                            None,
                        ),
                        loop_of(
                            a,
                            "flag",
                            flags,
                            vec![a.if_(
                                a.binary(
                                    a.id("flags"),
                                    B::BitwiseAnd,
                                    a.member(a.id("flag"), a.number(0.0)),
                                ),
                                vec![a.assign(
                                    a.id("text"),
                                    a.binary(
                                        a.id("text"),
                                        B::Addition,
                                        a.member(a.id("flag"), a.number(1.0)),
                                    ),
                                )],
                                None,
                            )],
                        ),
                        a.ret(Some(a.construct(
                            a.id("RegExp"),
                            vec![a.id("pattern"), a.id("text")],
                        ))),
                    ],
                )
            }
            I::PropertyKey => helper(
                a,
                *kind,
                &["value"],
                vec![a.ret(Some(a.member(
                    call(
                        a,
                        "Reflect",
                        "ownKeys",
                        vec![a.object(vec![(a.id("value"), a.number(0.0))])],
                    ),
                    a.number(0.0),
                )))],
            ),
            I::SetPrototype => helper(
                a,
                *kind,
                &["target", "proto"],
                vec![
                    a.if_(
                        Expression::new_logical_expression(
                            SPAN,
                            object(a, "proto"),
                            L::Or,
                            a.binary(
                                a.id("proto"),
                                B::StrictEquality,
                                Expression::new_null_literal(SPAN, &a.build),
                            ),
                            &a.build,
                        ),
                        vec![a.expr(call(
                            a,
                            "Object",
                            "setPrototypeOf",
                            vec![a.id("target"), a.id("proto")],
                        ))],
                        None,
                    ),
                    a.ret(Some(a.id("target"))),
                ],
            ),
            I::ObjectRest => helper(
                a,
                *kind,
                &["source", "excluded"],
                vec![
                    a.var("result", Some(a.object(vec![])), true),
                    a.if_(
                        a.binary(
                            a.id("source"),
                            B::Equality,
                            Expression::new_null_literal(SPAN, &a.build),
                        ),
                        vec![a.ret(Some(a.id("result")))],
                        None,
                    ),
                    a.var("keys", Some(a.construct(a.id("Set"), vec![])), true),
                    a.if_(
                        a.binary(
                            a.member(a.id("excluded"), a.number(0.0)),
                            B::StrictInequality,
                            a.undefined(),
                        ),
                        vec![loop_of(
                            a,
                            "key",
                            a.id("excluded"),
                            vec![a.expr(call(
                                a,
                                "keys",
                                "add",
                                vec![a.call(a.id("__ark_property_key"), vec![a.id("key")])],
                            ))],
                        )],
                        None,
                    ),
                    a.var(
                        "from",
                        Some(a.call(a.id("Object"), vec![a.id("source")])),
                        true,
                    ),
                    loop_of(
                        a,
                        "key",
                        call(a, "Reflect", "ownKeys", vec![a.id("from")]),
                        vec![a.if_(
                            a.unary(U::LogicalNot, call(a, "keys", "has", vec![a.id("key")])),
                            vec![
                                a.var(
                                    "desc",
                                    Some(call(
                                        a,
                                        "Object",
                                        "getOwnPropertyDescriptor",
                                        vec![a.id("from"), a.id("key")],
                                    )),
                                    true,
                                ),
                                a.if_(
                                    Expression::new_logical_expression(
                                        SPAN,
                                        a.id("desc"),
                                        L::And,
                                        a.property(a.id("desc"), "enumerable"),
                                        &a.build,
                                    ),
                                    vec![a.expr(call(
                                        a,
                                        "Object",
                                        "defineProperty",
                                        vec![
                                            a.id("result"),
                                            a.id("key"),
                                            a.object(vec![
                                                (
                                                    a.string("value"),
                                                    a.member(a.id("from"), a.id("key")),
                                                ),
                                                (a.string("writable"), a.boolean(true)),
                                                (a.string("enumerable"), a.boolean(true)),
                                                (a.string("configurable"), a.boolean(true)),
                                            ]),
                                        ],
                                    ))],
                                    None,
                                ),
                            ],
                            None,
                        )],
                    ),
                    a.ret(Some(a.id("result"))),
                ],
            ),
            I::IteratorNext => helper(
                a,
                *kind,
                &["iterator", "method"],
                vec![
                    a.var(
                        "result",
                        Some(call(
                            a,
                            "Reflect",
                            "apply",
                            vec![a.id("method"), a.id("iterator"), a.array(vec![])],
                        )),
                        true,
                    ),
                    a.if_(
                        a.unary(U::LogicalNot, object(a, "result")),
                        vec![error(a, "TypeError", "Inner return result is not object")],
                        None,
                    ),
                    a.ret(Some(a.id("result"))),
                ],
            ),
            I::CloseIterator => helper(
                a,
                *kind,
                &["iterator"],
                vec![
                    a.var("method", Some(a.property(a.id("iterator"), "return")), true),
                    a.if_(
                        a.binary(
                            a.id("method"),
                            B::Equality,
                            Expression::new_null_literal(SPAN, &a.build),
                        ),
                        vec![a.ret(Some(a.undefined()))],
                        None,
                    ),
                    a.expr(a.call(
                        a.id("__ark_iterator_next"),
                        vec![a.id("iterator"), a.id("method")],
                    )),
                    a.ret(Some(a.undefined())),
                ],
            ),
            I::Template => {
                let mut body = vec![
                    a.var("raw", Some(a.member(a.id("literal"), a.number(0.0))), true),
                    a.if_(
                        call(a, "__ark_templates", "has", vec![a.id("raw")]),
                        vec![a.ret(Some(call(a, "__ark_templates", "get", vec![a.id("raw")])))],
                        None,
                    ),
                    a.var(
                        "cooked",
                        Some(a.member(a.id("literal"), a.number(1.0))),
                        true,
                    ),
                    a.var("result", Some(a.array(vec![])), true),
                    a.var("rawCopy", Some(a.array(vec![])), true),
                    a.var("i", Some(a.number(0.0)), false),
                ];
                let mut step = vec![];
                for (target, source) in [("result", "cooked"), ("rawCopy", "raw")] {
                    step.push(a.expr(call(
                        a,
                        "Object",
                        "defineProperty",
                        vec![
                            a.id(target),
                            a.id("i"),
                            a.object(vec![
                                (a.string("value"), a.member(a.id(source), a.id("i"))),
                                (a.string("writable"), a.boolean(true)),
                            ]),
                        ],
                    )));
                }
                step.push(a.assign(a.id("i"), a.binary(a.id("i"), B::Addition, a.number(1.0))));
                body.push(Statement::new_while_statement(
                    SPAN,
                    a.binary(a.id("i"), B::LessThan, a.property(a.id("cooked"), "length")),
                    a.block(step),
                    &a.build,
                ));
                body.push(a.expr(call(
                    a,
                    "Object",
                    "defineProperty",
                    vec![
                        a.id("result"),
                        a.string("raw"),
                        a.object(vec![(
                            a.string("value"),
                            call(a, "Object", "freeze", vec![a.id("rawCopy")]),
                        )]),
                    ],
                )));
                body.push(a.expr(call(a, "Object", "freeze", vec![a.id("result")])));
                body.push(a.expr(call(
                    a,
                    "__ark_templates",
                    "set",
                    vec![a.id("raw"), a.id("result")],
                )));
                body.push(a.ret(Some(a.id("result"))));
                helper(a, *kind, &["literal"], body)
            }
            I::AsyncIterator => async_iterator(a),
            I::GlobalGet => helper(
                a,
                *kind,
                &["key"],
                vec![
                    a.if_(
                        call(a, "__ark_globals", "has", vec![a.id("key")]),
                        vec![a.ret(Some(a.property(
                            call(a, "__ark_globals", "get", vec![a.id("key")]),
                            "value",
                        )))],
                        None,
                    ),
                    a.if_(
                        a.unary(
                            U::LogicalNot,
                            a.binary(a.id("key"), B::In, a.id("globalThis")),
                        ),
                        vec![error(a, "ReferenceError", "Global binding is not defined")],
                        None,
                    ),
                    a.ret(Some(a.member(a.id("globalThis"), a.id("key")))),
                ],
            ),
            I::GlobalDeclare => helper(
                a,
                *kind,
                &["key", "value", "constant"],
                vec![
                    a.if_(
                        call(a, "__ark_globals", "has", vec![a.id("key")]),
                        vec![error(
                            a,
                            "SyntaxError",
                            "Global binding has already been declared",
                        )],
                        None,
                    ),
                    a.expr(call(
                        a,
                        "__ark_globals",
                        "set",
                        vec![
                            a.id("key"),
                            a.object(vec![
                                (a.string("value"), a.id("value")),
                                (a.string("constant"), a.id("constant")),
                            ]),
                        ],
                    )),
                ],
            ),
            I::GlobalSet => helper(
                a,
                *kind,
                &["key", "value", "strict"],
                vec![
                    a.if_(
                        call(a, "__ark_globals", "has", vec![a.id("key")]),
                        vec![
                            a.var(
                                "binding",
                                Some(call(a, "__ark_globals", "get", vec![a.id("key")])),
                                true,
                            ),
                            a.if_(
                                a.property(a.id("binding"), "constant"),
                                vec![error(a, "TypeError", "Assignment to constant variable")],
                                None,
                            ),
                            a.assign(a.property(a.id("binding"), "value"), a.id("value")),
                            a.ret(None),
                        ],
                        None,
                    ),
                    a.if_(
                        Expression::new_logical_expression(
                            SPAN,
                            a.id("strict"),
                            L::And,
                            a.unary(
                                U::LogicalNot,
                                a.binary(a.id("key"), B::In, a.id("globalThis")),
                            ),
                            &a.build,
                        ),
                        vec![error(a, "ReferenceError", "Global binding is not defined")],
                        None,
                    ),
                    a.assign(a.member(a.id("globalThis"), a.id("key")), a.id("value")),
                ],
            ),
            I::PatchGet => helper(
                a,
                *kind,
                &["index"],
                vec![a.ret(Some(a.member(a.id("__ark_patches"), a.id("index"))))],
            ),
            I::PatchSet => helper(
                a,
                *kind,
                &["index", "value"],
                vec![a.assign(
                    a.member(a.id("__ark_patches"), a.id("index")),
                    a.id("value"),
                )],
            ),
            I::OwnNamed => helper(
                a,
                *kind,
                &["target", "key", "value"],
                vec![
                    a.assign(
                        a.id("key"),
                        a.call(a.id("__ark_property_key"), vec![a.id("key")]),
                    ),
                    a.expr(call(
                        a,
                        "Object",
                        "defineProperty",
                        vec![
                            a.id("value"),
                            a.string("name"),
                            a.object(vec![(
                                a.string("value"),
                                a.call(a.id("String"), vec![a.id("key")]),
                            )]),
                        ],
                    )),
                    a.expr(a.call(
                        a.id("__ark_define_own"),
                        vec![a.id("target"), a.id("key"), a.id("value")],
                    )),
                ],
            ),
        };
        out.push(statement);
    }
    out
}
fn async_iterator<'a>(a: &Ast<'a>) -> Statement<'a> {
    let mut body = vec![
        a.var(
            "method",
            Some(a.member(a.id("object"), a.property(a.id("Symbol"), "asyncIterator"))),
            true,
        ),
        a.if_(
            a.binary(
                a.id("method"),
                B::Inequality,
                Expression::new_null_literal(SPAN, &a.build),
            ),
            vec![a.ret(Some(a.call(
                a.id("__ark_iterator_next"),
                vec![a.id("object"), a.id("method")],
            )))],
            None,
        ),
        a.var(
            "sync",
            Some(a.call(a.id("__ark_get_iterator"), vec![a.id("object")])),
            true,
        ),
        a.var("syncNext", Some(a.property(a.id("sync"), "next")), true),
        a.var("result", Some(a.object(vec![])), true),
    ];
    for name in ["next", "return", "throw"] {
        let mut step = vec![a.var(
            "method",
            Some(if name == "next" {
                a.id("syncNext")
            } else {
                a.property(a.id("sync"), name)
            }),
            true,
        )];
        if name != "next" {
            step.push(a.if_(
                a.binary(
                    a.id("method"),
                    B::Equality,
                    Expression::new_null_literal(SPAN, &a.build),
                ),
                if name == "return" {
                    vec![a.ret(Some(a.object(vec![
                        (a.string("value"), a.id("value")),
                        (a.string("done"), a.boolean(true)),
                    ])))]
                } else {
                    vec![
                        a.expr(a.call(a.id("__ark_close_iterator"), vec![a.id("sync")])),
                        error(a, "TypeError", "Throw method is not defined"),
                    ]
                },
                None,
            ));
        }
        step.push(a.var(
            "step",
            Some(call(
                a,
                "Reflect",
                "apply",
                vec![a.id("method"), a.id("sync"), a.id("arguments")],
            )),
            true,
        ));
        step.push(a.if_(
            a.unary(U::LogicalNot, object(a, "step")),
            vec![error(a, "TypeError", "Iterator result is not an object")],
            None,
        ));
        step.push(a.var(
            "done",
            Some(a.unary(
                U::LogicalNot,
                a.unary(U::LogicalNot, a.property(a.id("step"), "done")),
            )),
            true,
        ));
        step.push(a.var(
            "resolved",
            Some(Expression::new_await_expression(
                SPAN,
                a.property(a.id("step"), "value"),
                &a.build,
            )),
            true,
        ));
        step.push(a.ret(Some(a.object(vec![
            (a.string("value"), a.id("resolved")),
            (a.string("done"), a.id("done")),
        ]))));
        body.push(a.assign(
            a.property(a.id("result"), name),
            a.function(
                &format!("__ark_async_{name}"),
                vec!["value".into()],
                step,
                true,
            ),
        ));
    }
    body.push(a.assign(
        a.member(a.id("result"), a.property(a.id("Symbol"), "asyncIterator")),
        a.function(
            "__ark_async_iterator_self",
            vec![],
            vec![a.ret(Some(Expression::new_this_expression(SPAN, &a.build)))],
            false,
        ),
    ));
    body.push(a.ret(Some(a.id("result"))));
    helper(a, I::AsyncIterator, &["object"], body)
}
