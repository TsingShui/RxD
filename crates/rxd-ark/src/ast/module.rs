use super::{lower::string_literal, nodes::Ast};
use crate::{Module, reader::invalid};
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_str::Ident;
use rxd_core::Result;

pub(super) fn local_index(module: &Module, index: usize) -> usize {
    let name = &module.local_exports[index].local_name.units;
    module
        .local_exports
        .iter()
        .position(|entry| &entry.local_name.units == name)
        .unwrap_or(index)
}

pub(super) fn preamble<'a>(ast: &Ast<'a>, module: &Module) -> Vec<Statement<'a>> {
    let context = ast.object(vec![
        (
            ast.string("env"),
            Expression::new_null_literal(SPAN, &ast.build),
        ),
        (
            ast.string("hole"),
            ast.call(
                ast.property(ast.id("Symbol"), "for"),
                vec![ast.string("rxd.ark.hole")],
            ),
        ),
        (ast.string("locals"), ast.object(Vec::new())),
    ]);
    let mut statements = vec![ast.var("__ark_module_context", Some(context), true)];
    for index in 0..module.local_exports.len() {
        let original = local_index(module, index);
        if original != index {
            continue;
        }
        statements.push(ast.var(
            &format!("__ark_local_{original}"),
            Some(ast.property(ast.id("__ark_module_context"), "hole")),
            false,
        ));
    }
    statements
}

pub(super) fn finish<'a>(ast: &Ast<'a>, module: &Module, main: u32) -> Result<Vec<Statement<'a>>> {
    let factory = ast.call(
        ast.id(&format!("_ark_capture_{main:x}")),
        vec![ast.id("__ark_module_context")],
    );
    let mut statements = vec![ast.expr(ast.call(factory, Vec::new()))];
    for (index, export) in module.local_exports.iter().enumerate() {
        let index = local_index(module, index);
        let reference = IdentifierReference::new(
            SPAN,
            Ident::from_str_in(&format!("__ark_local_{index}"), &ast.build),
            &ast.build,
        );
        let specifier = ExportSpecifier::new(
            SPAN,
            ModuleExportName::IdentifierReference(reference),
            ModuleExportName::StringLiteral(string_literal(ast, &export.export_name)),
            ImportOrExportKind::Value,
            &ast.build,
        );
        statements.push(Statement::new_export_named_declaration(
            SPAN,
            ast.vec([specifier]),
            ImportOrExportKind::Value,
            &ast.build,
        ));
    }
    for export in &module.indirect_exports {
        let request = export
            .request
            .and_then(|index| module.requests.get(index as usize))
            .ok_or_else(|| invalid("Ark indirect export request is missing"))?;
        let specifier = ExportSpecifier::new(
            SPAN,
            ModuleExportName::StringLiteral(string_literal(ast, &export.local_name)),
            ModuleExportName::StringLiteral(string_literal(ast, &export.export_name)),
            ImportOrExportKind::Value,
            &ast.build,
        );
        statements.push(Statement::new_export_from_declaration(
            SPAN,
            ast.vec([specifier]),
            string_literal(ast, request),
            ImportOrExportKind::Value,
            None,
            &ast.build,
        ));
    }
    for index in &module.star_exports {
        let request = module
            .requests
            .get(*index as usize)
            .ok_or_else(|| invalid("Ark star export request is missing"))?;
        statements.push(Statement::new_export_all_declaration(
            SPAN,
            None,
            string_literal(ast, request),
            None,
            ImportOrExportKind::Value,
            &ast.build,
        ));
    }
    Ok(statements)
}
