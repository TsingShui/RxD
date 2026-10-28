use super::{extra::call, nodes::Ast};
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::BinaryOperator as B;

pub(super) fn helpers<'a>(a: &Ast<'a>) -> Vec<Statement<'a>> {
    let mut out = vec![
        a.var("__ark_sendable_root", Some(a.undefined()), false),
        a.var(
            "__ark_sendable_layouts",
            Some(a.construct(a.id("WeakMap"), vec![])),
            true,
        ),
        a.var("__ark_concurrent_results", Some(a.array(vec![])), true),
    ];
    out.push(a.factory(
        "__ark_sendable_env",
        vec!["size".into(), "hole".into()],
        vec![
            a.assign(
                a.id("__ark_sendable_root"),
                a.object(vec![
                    (a.string("parent"), a.id("__ark_sendable_root")),
                    (
                        a.string("slots"),
                        a.call(
                            a.property(a.call(a.id("Array"), vec![a.id("size")]), "fill"),
                            vec![a.id("hole")],
                        ),
                    ),
                ]),
            ),
            a.ret(Some(a.id("__ark_sendable_root"))),
        ],
    ));
    out.push(a.factory(
        "__ark_sendable_at",
        vec!["depth".into()],
        vec![
            a.var("env", Some(a.id("__ark_sendable_root")), false),
            Statement::new_while_statement(
                SPAN,
                a.binary(a.id("depth"), B::GreaterThan, a.number(0.0)),
                a.block(vec![
                    a.assign(a.id("env"), a.property(a.id("env"), "parent")),
                    a.assign(
                        a.id("depth"),
                        a.binary(a.id("depth"), B::Subtraction, a.number(1.0)),
                    ),
                ]),
                &a.build,
            ),
            a.ret(Some(a.id("env"))),
        ],
    ));
    let cell = || {
        a.member(
            a.property(
                a.call(a.id("__ark_sendable_at"), vec![a.id("depth")]),
                "slots",
            ),
            a.id("index"),
        )
    };
    out.push(a.factory(
        "__ark_sendable_get",
        vec!["depth".into(), "index".into()],
        vec![a.ret(Some(cell()))],
    ));
    out.push(a.factory(
        "__ark_sendable_set",
        vec!["depth".into(), "index".into(), "value".into()],
        vec![a.assign(cell(), a.id("value"))],
    ));
    out.push(a.factory(
        "__ark_sendable_load_class",
        vec!["depth".into()],
        vec![a.ret(Some(a.property(
            a.call(a.id("__ark_sendable_at"), vec![a.id("depth")]),
            "class",
        )))],
    ));
    out.push(a.factory(
        "__ark_sendable_class",
        vec!["constructor".into(), "layout".into()],
        vec![
            a.expr(call(
                a,
                "__ark_sendable_layouts",
                "set",
                vec![a.id("constructor"), a.id("layout")],
            )),
            a.if_(
                a.id("__ark_sendable_root"),
                vec![a.assign(
                    a.property(a.id("__ark_sendable_root"), "class"),
                    a.id("constructor"),
                )],
                None,
            ),
            a.ret(Some(a.id("constructor"))),
        ],
    ));
    out.push(a.factory(
        "__ark_concurrent_result",
        vec!["value".into()],
        vec![a.expr(call(
            a,
            "__ark_concurrent_results",
            "push",
            vec![a.id("value")],
        ))],
    ));
    out
}
