use super::{extra::call, nodes::Ast};
use oxc_ast::ast::*;
use oxc_span::SPAN;

pub(super) fn helpers<'a>(a: &Ast<'a>) -> Vec<Statement<'a>> {
    let capability = a.function(
        "__ark_capability",
        vec!["resolve".into(), "reject".into()],
        vec![
            a.assign(a.property(a.id("target"), "resolve"), a.id("resolve")),
            a.assign(a.property(a.id("target"), "reject"), a.id("reject")),
        ],
        false,
    );
    let enter = a.factory(
        "__ark_async_enter",
        vec![],
        vec![
            a.var("target", Some(a.object(vec![])), true),
            a.assign(
                a.property(a.id("target"), "promise"),
                a.construct(a.id("Promise"), vec![capability]),
            ),
            a.expr(call(
                a,
                "__ark_generator_state",
                "set",
                vec![
                    a.id("target"),
                    a.object(vec![
                        (a.string("mode"), a.number(2.0)),
                        (a.string("value"), a.undefined()),
                    ]),
                ],
            )),
            a.ret(Some(a.id("target"))),
        ],
    );
    let mut wait = a.factory(
        "__ark_async_await",
        vec!["target".into(), "value".into()],
        vec![
            a.var(
                "state",
                Some(call(
                    a,
                    "__ark_generator_state",
                    "get",
                    vec![a.id("target")],
                )),
                true,
            ),
            a.try_catch(
                vec![
                    a.assign(
                        a.property(a.id("state"), "value"),
                        Expression::new_await_expression(SPAN, a.id("value"), &a.build),
                    ),
                    a.assign(a.property(a.id("state"), "mode"), a.number(2.0)),
                ],
                "error",
                vec![
                    a.assign(a.property(a.id("state"), "value"), a.id("error")),
                    a.assign(a.property(a.id("state"), "mode"), a.number(1.0)),
                ],
            ),
            a.ret(Some(a.object(vec![(
                a.string("promise"),
                a.property(a.id("target"), "promise"),
            )]))),
        ],
    );
    if let Statement::FunctionDeclaration(wait) = &mut wait {
        wait.r#async = true;
    }
    let mut result = vec![enter, wait];
    for name in ["resolve", "reject"] {
        result.push(a.factory(
            &format!("__ark_async_{name}"),
            vec!["target".into(), "value".into()],
            vec![
                a.expr(a.call(a.property(a.id("target"), name), vec![a.id("value")])),
                a.ret(Some(a.property(a.id("target"), "promise"))),
            ],
        ));
    }
    result.push(a.factory(
        "__ark_async_gen_reject",
        vec!["target".into(), "error".into()],
        vec![
            a.var(
                "state",
                Some(call(
                    a,
                    "__ark_generator_state",
                    "get",
                    vec![a.id("target")],
                )),
                true,
            ),
            a.assign(a.property(a.id("state"), "failed"), a.boolean(true)),
            a.assign(a.property(a.id("state"), "error"), a.id("error")),
        ],
    ));
    result.push(a.factory(
        "__ark_async_pause",
        vec!["target".into(), "value".into()],
        vec![
            a.assign(
                a.property(
                    call(a, "__ark_generator_state", "get", vec![a.id("target")]),
                    "suspendedValue",
                ),
                a.id("value"),
            ),
            a.ret(Some(a.id("value"))),
        ],
    ));
    result.push(a.factory(
        "__ark_generator_state_set",
        vec!["target".into(), "value".into()],
        vec![a.assign(
            a.property(
                call(a, "__ark_generator_state", "get", vec![a.id("target")]),
                "state",
            ),
            a.id("value"),
        )],
    ));
    result
}
