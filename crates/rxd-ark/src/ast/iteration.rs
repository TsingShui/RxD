use super::nodes::Ast;
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::{BinaryOperator as B, LogicalOperator as L, UnaryOperator as U};

pub(super) fn property_iterator<'a>(ast: &Ast<'a>) -> Statement<'a> {
    let binding = ForStatementLeft::new_variable_declaration(
        SPAN,
        VariableDeclarationKind::Const,
        ast.vec([VariableDeclarator::new(
            SPAN,
            ast.pattern("key"),
            None,
            None,
            false,
            &ast.build,
        )]),
        false,
        &ast.build,
    );
    let yield_key = Expression::new_yield_expression(SPAN, false, Some(ast.id("key")), &ast.build);
    let body = Statement::new_for_in_statement(
        SPAN,
        binding,
        ast.id("object"),
        ast.block(vec![ast.expr(yield_key)]),
        &ast.build,
    );
    let mut function = ast.factory("__ark_property_iterator", vec!["object".into()], vec![body]);
    if let Statement::FunctionDeclaration(function) = &mut function {
        function.generator = true;
    }
    function
}

pub(super) fn apply<'a>(ast: &Ast<'a>) -> Statement<'a> {
    let object = Expression::new_logical_expression(
        SPAN,
        ast.binary(
            ast.id("receiver"),
            B::StrictInequality,
            Expression::new_null_literal(SPAN, &ast.build),
        ),
        L::And,
        Expression::new_logical_expression(
            SPAN,
            ast.binary(
                ast.unary(U::Typeof, ast.id("receiver")),
                B::StrictEquality,
                ast.string("object"),
            ),
            L::Or,
            ast.binary(
                ast.unary(U::Typeof, ast.id("receiver")),
                B::StrictEquality,
                ast.string("function"),
            ),
            &ast.build,
        ),
        &ast.build,
    );
    let valid_receiver = Expression::new_logical_expression(
        SPAN,
        ast.binary(ast.id("receiver"), B::StrictEquality, ast.undefined()),
        L::Or,
        object,
        &ast.build,
    );
    let valid = Expression::new_logical_expression(
        SPAN,
        valid_receiver,
        L::And,
        Expression::new_logical_expression(
            SPAN,
            ast.binary(
                ast.unary(U::Typeof, ast.id("fn")),
                B::StrictEquality,
                ast.string("function"),
            ),
            L::And,
            ast.call(
                ast.property(ast.id("Array"), "isArray"),
                vec![ast.id("args")],
            ),
            &ast.build,
        ),
        &ast.build,
    );
    let args = Expression::new_array_expression(
        SPAN,
        ast.vec([ArrayExpressionElement::new_spread_element(
            SPAN,
            ast.id("args"),
            &ast.build,
        )]),
        &ast.build,
    );
    ast.factory(
        "__ark_apply",
        vec!["fn".into(), "receiver".into(), "args".into()],
        vec![
            ast.if_(
                ast.unary(U::LogicalNot, valid),
                vec![ast.throw(
                    ast.construct(ast.id("TypeError"), vec![ast.string("cannot Callspread")]),
                )],
                None,
            ),
            ast.ret(Some(ast.call(
                ast.property(ast.id("Reflect"), "apply"),
                vec![ast.id("fn"), ast.id("receiver"), args],
            ))),
        ],
    )
}

pub(super) fn array_spread<'a>(ast: &Ast<'a>) -> Statement<'a> {
    let error = |message| ast.throw(ast.construct(ast.id("TypeError"), vec![ast.string(message)]));
    let mut body = vec![ast.if_(
        ast.unary(
            U::LogicalNot,
            ast.call(
                ast.property(ast.id("Array"), "isArray"),
                vec![ast.id("dst")],
            ),
        ),
        vec![error("dst is not iterable")],
        None,
    )];
    let nullish = ast.binary(
        ast.id("src"),
        B::Equality,
        Expression::new_null_literal(SPAN, &ast.build),
    );
    body.push(ast.if_(nullish, vec![error("src is not iterable")], None));
    let string_body = vec![
        ast.var("i", Some(ast.number(0.0)), false),
        Statement::new_while_statement(
            SPAN,
            ast.binary(
                ast.id("i"),
                B::LessThan,
                ast.property(ast.id("src"), "length"),
            ),
            ast.block(vec![
                ast.assign(
                    ast.member(
                        ast.id("dst"),
                        ast.binary(ast.id("index"), B::Addition, ast.id("i")),
                    ),
                    ast.member(ast.id("src"), ast.id("i")),
                ),
                ast.assign(
                    ast.id("i"),
                    ast.binary(ast.id("i"), B::Addition, ast.number(1.0)),
                ),
            ]),
            &ast.build,
        ),
        ast.if_(
            ast.binary(ast.id("i"), B::StrictEquality, ast.number(0.0)),
            vec![ast.assign(ast.property(ast.id("dst"), "length"), ast.id("index"))],
            None,
        ),
        ast.ret(Some(ast.binary(ast.id("index"), B::Addition, ast.id("i")))),
    ];
    body.push(ast.if_(
        ast.binary(
            ast.unary(U::Typeof, ast.id("src")),
            B::StrictEquality,
            ast.string("string"),
        ),
        string_body,
        None,
    ));
    body.push(ast.var(
        "iterator",
        Some(ast.call(ast.id("__ark_get_iterator"), vec![ast.id("src")])),
        true,
    ));
    body.push(ast.var("written", Some(ast.number(0.0)), false));
    let result_invalid = Expression::new_logical_expression(
        SPAN,
        ast.binary(
            ast.id("step"),
            B::StrictEquality,
            Expression::new_null_literal(SPAN, &ast.build),
        ),
        L::Or,
        Expression::new_logical_expression(
            SPAN,
            ast.binary(
                ast.unary(U::Typeof, ast.id("step")),
                B::StrictInequality,
                ast.string("object"),
            ),
            L::And,
            ast.binary(
                ast.unary(U::Typeof, ast.id("step")),
                B::StrictInequality,
                ast.string("function"),
            ),
            &ast.build,
        ),
        &ast.build,
    );
    let descriptor = ast.call(
        ast.property(ast.id("Object"), "getOwnPropertyDescriptor"),
        vec![ast.id("step"), ast.string("value")],
    );
    let enumerable = Expression::new_logical_expression(
        SPAN,
        ast.id("desc"),
        L::And,
        ast.property(ast.id("desc"), "enumerable"),
        &ast.build,
    );
    let loop_body = vec![
        ast.var(
            "step",
            Some(ast.call(ast.property(ast.id("iterator"), "next"), vec![])),
            true,
        ),
        ast.if_(
            result_invalid,
            vec![error("Inner return result is not object")],
            None,
        ),
        ast.if_(
            ast.property(ast.id("step"), "done"),
            vec![ast.jump("__ark_spread", false)],
            None,
        ),
        ast.var("desc", Some(descriptor), true),
        ast.if_(
            enumerable,
            vec![
                ast.expr(ast.call(
                    ast.property(ast.id("Reflect"), "defineProperty"),
                    vec![ast.id("dst"), ast.id("index"), ast.id("desc")],
                )),
                ast.assign(
                    ast.id("index"),
                    ast.binary(ast.id("index"), B::Addition, ast.number(1.0)),
                ),
                ast.assign(
                    ast.id("written"),
                    ast.binary(ast.id("written"), B::Addition, ast.number(1.0)),
                ),
            ],
            None,
        ),
    ];
    body.push(ast.label(
        "__ark_spread",
        Statement::new_while_statement(SPAN, ast.boolean(true), ast.block(loop_body), &ast.build),
    ));
    body.push(ast.if_(
        ast.binary(ast.id("written"), B::StrictEquality, ast.number(0.0)),
        vec![ast.assign(ast.property(ast.id("dst"), "length"), ast.id("index"))],
        None,
    ));
    body.push(ast.ret(Some(ast.id("index"))));
    ast.factory(
        "__ark_array_spread",
        vec!["dst".into(), "index".into(), "src".into()],
        body,
    )
}
