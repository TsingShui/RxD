use super::{
    extra::{call, error, object},
    nodes::Ast,
};
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::{BinaryOperator as B, LogicalOperator as L, UnaryOperator as U};

pub(super) fn helpers<'a>(a: &Ast<'a>) -> Vec<Statement<'a>> {
    let mut out = vec![a.var(
        "__ark_private_fields",
        Some(a.construct(a.id("WeakMap"), vec![])),
        true,
    )];
    for (name, write, define, test) in [
        ("private_get", false, false, false),
        ("private_set", true, false, false),
        ("private_define", true, true, false),
        ("private_has", false, false, true),
    ] {
        let mut body = vec![a.if_(
            a.unary(U::LogicalNot, object(a, "target")),
            vec![error(a, "TypeError", "Private receiver is not an object")],
            None,
        )];
        if !test && !define {
            body.push(a.if_(
                a.binary(
                    a.unary(U::Typeof, a.id("key")),
                    B::StrictEquality,
                    a.string("function"),
                ),
                vec![a.ret(Some(call(
                    a,
                    "Reflect",
                    "apply",
                    vec![
                        a.id("key"),
                        a.id("target"),
                        a.array(if write { vec![a.id("value")] } else { vec![] }),
                    ],
                )))],
                None,
            ));
        }
        body.push(a.var(
            "fields",
            Some(call(a, "__ark_private_fields", "get", vec![a.id("target")])),
            false,
        ));
        if test {
            body.push(a.ret(Some(a.unary(
                U::LogicalNot,
                a.unary(
                    U::LogicalNot,
                    Expression::new_logical_expression(
                        SPAN,
                        a.id("fields"),
                        L::And,
                        call(a, "fields", "has", vec![a.id("key")]),
                        &a.build,
                    ),
                ),
            ))));
        } else {
            if define {
                body.push(a.if_(
                    a.unary(U::LogicalNot, a.id("fields")),
                    vec![
                        a.assign(a.id("fields"), a.construct(a.id("Map"), vec![])),
                        a.expr(call(
                            a,
                            "__ark_private_fields",
                            "set",
                            vec![a.id("target"), a.id("fields")],
                        )),
                    ],
                    None,
                ));
            } else {
                body.push(a.if_(
                    Expression::new_logical_expression(
                        SPAN,
                        a.unary(U::LogicalNot, a.id("fields")),
                        L::Or,
                        a.unary(U::LogicalNot, call(a, "fields", "has", vec![a.id("key")])),
                        &a.build,
                    ),
                    vec![error(a, "TypeError", "invalid or cannot find private key")],
                    None,
                ));
            }
            if write {
                body.push(a.expr(call(a, "fields", "set", vec![a.id("key"), a.id("value")])));
                body.push(a.ret(Some(a.boolean(true))));
            } else {
                body.push(a.ret(Some(call(a, "fields", "get", vec![a.id("key")]))));
            }
        }
        out.push(a.factory(
            &format!("__ark_{name}"),
            if write {
                vec!["key".into(), "target".into(), "value".into()]
            } else {
                vec!["key".into(), "target".into()]
            },
            body,
        ));
    }
    let slots = || a.property(a.id("env"), "slots");
    let index = || a.member(slots(), a.id("index"));
    let mut body = vec![
        a.var("index", Some(a.number(0.0)), false),
        Statement::new_while_statement(
            SPAN,
            Expression::new_logical_expression(
                SPAN,
                a.binary(a.id("index"), B::LessThan, a.property(slots(), "length")),
                L::And,
                a.binary(index(), B::StrictInequality, a.id("hole")),
                &a.build,
            ),
            a.block(vec![a.assign(
                a.id("index"),
                a.binary(a.id("index"), B::Addition, a.number(1.0)),
            )]),
            &a.build,
        ),
        a.var("i", Some(a.number(0.0)), false),
        Statement::new_while_statement(
            SPAN,
            a.binary(a.id("i"), B::LessThan, a.id("count")),
            a.block(vec![
                a.assign(index(), a.call(a.id("Symbol"), vec![])),
                a.assign(
                    a.id("index"),
                    a.binary(a.id("index"), B::Addition, a.number(1.0)),
                ),
                a.assign(a.id("i"), a.binary(a.id("i"), B::Addition, a.number(1.0))),
            ]),
            &a.build,
        ),
        a.assign(a.id("i"), a.number(0.0)),
        Statement::new_while_statement(
            SPAN,
            a.binary(
                a.id("i"),
                B::LessThan,
                a.binary(
                    a.property(a.id("literal"), "length"),
                    B::Subtraction,
                    a.number(1.0),
                ),
            ),
            a.block(vec![
                a.assign(index(), a.member(a.id("literal"), a.id("i"))),
                a.assign(
                    a.id("index"),
                    a.binary(a.id("index"), B::Addition, a.number(1.0)),
                ),
                a.assign(a.id("i"), a.binary(a.id("i"), B::Addition, a.number(1.0))),
            ]),
            &a.build,
        ),
    ];
    body.push(a.if_(
        a.binary(
            a.member(a.id("literal"), a.id("i")),
            B::GreaterThan,
            a.number(0.0),
        ),
        vec![a.assign(index(), a.call(a.id("Symbol"), vec![a.string("method")]))],
        None,
    ));
    out.push(a.factory(
        "__ark_private_keys",
        vec![
            "env".into(),
            "count".into(),
            "literal".into(),
            "hole".into(),
        ],
        body,
    ));
    out
}
