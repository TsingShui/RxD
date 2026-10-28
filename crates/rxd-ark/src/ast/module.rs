use super::{
    lower::{SourceWarnings, string_literal},
    nodes::Ast,
};
use crate::{
    Module, Name,
    reader::{invalid, resource},
};
use oxc_ast::ast::*;
use oxc_span::SPAN;
use oxc_str::Ident;
use rxd_core::Result;
use std::cell::RefCell;

pub(super) fn local_index(module: &Module, index: usize) -> usize {
    let name = &module.local_exports[index].local_name.units;
    module
        .local_exports
        .iter()
        .position(|entry| &entry.local_name.units == name)
        .unwrap_or(index)
}

pub(super) fn import_name(record: Option<u32>, index: u32) -> String {
    record.map_or_else(
        || format!("__ark_i_{index}"),
        |at| format!("__ark_r{at:x}_i_{index}"),
    )
}

pub(super) fn namespace_name(record: Option<u32>, index: u32) -> String {
    record.map_or_else(
        || format!("__ark_ns_{index}"),
        |at| format!("__ark_r{at:x}_ns_{index}"),
    )
}

fn context_name(record: Option<u32>) -> String {
    record.map_or_else(
        || "__ark_module_context".to_owned(),
        |at| format!("__ark_r{at:x}_context"),
    )
}

pub(super) fn local_name(record: Option<u32>, index: usize) -> String {
    record.map_or_else(
        || format!("__ark_local_{index}"),
        |at| format!("__ark_r{at:x}_local_{index}"),
    )
}

fn scoped_export<'a>(
    ast: &Ast<'a>,
    record: u32,
    kind: &str,
    index: usize,
    name: Option<&Name>,
) -> Result<ModuleExportName<'a>> {
    let prefix = format!("record:0x{record:x}:{kind}:{index}:");
    let mut units = Vec::new();
    let encoded = name.is_some_and(|value| value.scalar_text().is_none());
    let name_units = name.map_or(0, |value| value.units.len());
    let name_size = if encoded {
        name_units
            .checked_mul(4)
            .and_then(|size| size.checked_add(6))
            .ok_or_else(|| resource("Ark export name size overflow"))?
    } else {
        name_units
    };
    let size = prefix
        .len()
        .checked_add(name_size)
        .ok_or_else(|| resource("Ark export name size overflow"))?;
    units
        .try_reserve_exact(size)
        .map_err(|_| resource("Ark export name allocation failed"))?;
    units.extend(prefix.encode_utf16());
    if let Some(name) = name {
        if encoded {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            units.extend("utf16:".encode_utf16());
            for unit in &name.units {
                for shift in [12, 8, 4, 0] {
                    units.push(u16::from(HEX[((unit >> shift) & 15) as usize]));
                }
            }
        } else {
            units.extend_from_slice(&name.units);
        }
    }
    let name = Name {
        offset: 0,
        end: 0,
        text: String::from_utf16_lossy(&units),
        units,
    };
    Ok(ModuleExportName::StringLiteral(string_literal(ast, &name)))
}

pub(super) fn preamble<'a>(
    ast: &Ast<'a>,
    module: &Module,
    record: Option<u32>,
) -> Vec<Statement<'a>> {
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
    let context_name = context_name(record);
    let mut statements = vec![ast.var(&context_name, Some(context), true)];
    for index in 0..module.local_exports.len() {
        let original = local_index(module, index);
        if original != index {
            continue;
        }
        statements.push(ast.var(
            &local_name(record, original),
            Some(ast.property(ast.id(&context_name), "hole")),
            false,
        ));
    }
    statements
}

pub(super) fn finish<'a>(
    ast: &Ast<'a>,
    module: &Module,
    main: u32,
    record: Option<u32>,
    warnings: &RefCell<SourceWarnings>,
) -> Result<Vec<Statement<'a>>> {
    let factory = ast.call(
        ast.id(&format!("_ark_capture_{main:x}")),
        vec![ast.id(&context_name(record))],
    );
    let mut statements = vec![ast.expr(ast.call(factory, Vec::new()))];
    for (ordinal, export) in module.local_exports.iter().enumerate() {
        let index = local_index(module, ordinal);
        let reference = IdentifierReference::new(
            SPAN,
            Ident::from_str_in(&local_name(record, index), &ast.build),
            &ast.build,
        );
        let specifier = ExportSpecifier::new(
            SPAN,
            ModuleExportName::IdentifierReference(reference),
            if let Some(record) = record {
                scoped_export(ast, record, "local", ordinal, Some(&export.export_name))?
            } else {
                ModuleExportName::StringLiteral(string_literal(ast, &export.export_name))
            },
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
    for (index, export) in module.indirect_exports.iter().enumerate() {
        let request = export
            .request
            .and_then(|index| module.requests.get(index as usize))
            .ok_or_else(|| invalid("Ark indirect export request is missing"))?;
        if let Some(record) = record
            && export.local_name.scalar_text().is_none()
        {
            let namespace = format!("__ark_r{record:x}_indirect_ns_{index}");
            let local = format!("__ark_r{record:x}_indirect_value_{index}");
            let specifier = ImportDeclarationSpecifier::new_import_namespace_specifier(
                SPAN,
                ast.binding(&namespace),
                &ast.build,
            );
            statements.push(Statement::new_import_declaration(
                SPAN,
                Some(ast.vec([specifier])),
                string_literal(ast, request),
                None,
                None,
                ImportOrExportKind::Value,
                &ast.build,
            ));
            let mut snapshot = ast.var(
                &local,
                Some(ast.member(ast.id(&namespace), ast.name(&export.local_name))),
                true,
            );
            warnings.borrow_mut().attach(
                &mut snapshot,
                crate::ReconstructionWarning::ModuleNameSnapshot,
            )?;
            statements.push(snapshot);
            let reference =
                IdentifierReference::new(SPAN, Ident::from_str_in(&local, &ast.build), &ast.build);
            let specifier = ExportSpecifier::new(
                SPAN,
                ModuleExportName::IdentifierReference(reference),
                scoped_export(ast, record, "indirect", index, Some(&export.export_name))?,
                ImportOrExportKind::Value,
                &ast.build,
            );
            statements.push(Statement::new_export_named_declaration(
                SPAN,
                ast.vec([specifier]),
                ImportOrExportKind::Value,
                &ast.build,
            ));
            continue;
        }
        let specifier = ExportSpecifier::new(
            SPAN,
            ModuleExportName::StringLiteral(string_literal(ast, &export.local_name)),
            if let Some(record) = record {
                scoped_export(ast, record, "indirect", index, Some(&export.export_name))?
            } else {
                ModuleExportName::StringLiteral(string_literal(ast, &export.export_name))
            },
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
    for (ordinal, index) in module.star_exports.iter().enumerate() {
        let request = module
            .requests
            .get(*index as usize)
            .ok_or_else(|| invalid("Ark star export request is missing"))?;
        statements.push(Statement::new_export_all_declaration(
            SPAN,
            record
                .map(|record| scoped_export(ast, record, "star", ordinal, None))
                .transpose()?,
            string_literal(ast, request),
            None,
            ImportOrExportKind::Value,
            &ast.build,
        ));
    }
    Ok(statements)
}
