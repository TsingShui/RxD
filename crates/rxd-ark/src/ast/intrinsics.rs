use super::nodes::Ast;
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_syntax::operator::{BinaryOperator as B, LogicalOperator as L};

// The pinned Ark runtime copies descriptors and ignores a false DefineOwnProperty result.
pub(super) fn copy_data<'a>(ast: &Ast<'a>) -> Statement<'a> {
    let mut body = vec![ast.var(
        "from",
        Some(ast.call(ast.id("Object"), vec![ast.id("s")])),
        true,
    )];
    let descriptor = ast.call(
        ast.property(ast.id("Object"), "getOwnPropertyDescriptor"),
        vec![ast.id("from"), ast.id("key")],
    );
    let get_descriptor = ast.var("desc", Some(descriptor), true);
    let fields = |data: bool| {
        let mut entries = if data {
            vec![
                (ast.string("value"), ast.property(ast.id("desc"), "value")),
                (ast.string("writable"), ast.boolean(true)),
            ]
        } else {
            vec![
                (ast.string("get"), ast.property(ast.id("desc"), "get")),
                (ast.string("set"), ast.property(ast.id("desc"), "set")),
            ]
        };
        entries.extend([
            (ast.string("enumerable"), ast.boolean(true)),
            (ast.string("configurable"), ast.boolean(true)),
        ]);
        ast.object(entries)
    };
    let kind = ast.binary(ast.string("value"), B::In, ast.id("desc"));
    let descriptor =
        Expression::new_conditional_expression(SPAN, kind, fields(true), fields(false), &ast.build);
    let define = ast.expr(ast.call(
        ast.property(ast.id("Reflect"), "defineProperty"),
        vec![ast.id("d"), ast.id("key"), descriptor],
    ));
    let enumerable = Expression::new_logical_expression(
        SPAN,
        ast.id("desc"),
        L::And,
        ast.property(ast.id("desc"), "enumerable"),
        &ast.build,
    );
    let loop_body = ast.block(vec![
        get_descriptor,
        ast.if_(enumerable, vec![define], None),
    ]);
    let decl = VariableDeclarator::new(SPAN, ast.pattern("key"), None, None, false, &ast.build);
    let binding = ForStatementLeft::new_variable_declaration(
        SPAN,
        VariableDeclarationKind::Const,
        ast.vec([decl]),
        false,
        &ast.build,
    );
    let keys = ast.call(
        ast.property(ast.id("Reflect"), "ownKeys"),
        vec![ast.id("from")],
    );
    body.push(Statement::new_for_of_statement(
        SPAN, false, binding, keys, loop_body, &ast.build,
    ));
    let nonnull = ast.binary(
        ast.id("s"),
        B::StrictInequality,
        Expression::new_null_literal(SPAN, &ast.build),
    );
    let defined = ast.binary(ast.id("s"), B::StrictInequality, ast.undefined());
    let test = Expression::new_logical_expression(SPAN, nonnull, L::And, defined, &ast.build);
    ast.factory(
        "__ark_copy",
        vec!["d".into(), "s".into()],
        vec![ast.if_(test, body, None), ast.ret(Some(ast.id("d")))],
    )
}
