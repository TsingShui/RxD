//! Bounded obfuscator.io string-array recovery.
//!
//! Detection follows webcrack's `string-array`, `array-rotator`, `decoder` and
//! `inline-decoded-strings` passes (MIT, j4k0xb). Only recognised factories,
//! literal decoder calls and their setup are rewritten; everything else keeps
//! its original bytes.

use std::time::{Duration, Instant};

use oxc_allocator::Allocator;
use oxc_ast::{AstKind, ast::*};
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::{Semantic, SymbolId};
use oxc_span::{GetSpan, SourceType, Span};
use rxd_core::{Error, Limits, Result, perf::Perf};

pub struct Outcome {
    pub code: String,
    pub decoded: usize,
    pub inlined: usize,
}

struct Edit {
    span: Span,
    text: String,
}

pub fn run(
    text: &str,
    source_type: SourceType,
    limits: Limits,
    perf: &mut Perf,
) -> Result<Outcome> {
    let allocator = Allocator::default();
    let parse_start = perf.start_stage();
    let parsed = Parser::new(&allocator, text, source_type)
        .with_options(ParseOptions {
            parse_regular_expression: true,
            ..ParseOptions::default()
        })
        .parse();
    perf.record("parse", parse_start);
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        let message = parsed
            .diagnostics
            .iter()
            .take(3)
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::new("INVALID_INPUT", message));
    }
    let program = parsed.program;
    let semantic_start = perf.start_stage();
    let semantic = crate::lexical::build(&program, limits, perf)?;
    perf.record("deobfuscate_scan", semantic_start);

    let mut edits = Vec::new();
    let inlined = inline_simple_arrays(&semantic, &mut edits);

    let Some(factory) = find_string_array(&semantic) else {
        let code = apply(text, edits)?;
        return Ok(Outcome {
            code,
            decoded: 0,
            inlined,
        });
    };

    let decoders = find_decoders(&semantic, factory.symbol);
    let rotator = find_rotator(&semantic, text, factory.symbol);

    let mut setup = vec![slice(text, factory.span)?];
    for decoder in &decoders {
        setup.push(slice(text, decoder.span)?);
    }
    if let Some(rotator) = rotator {
        setup.push(slice(text, rotator)?);
    }
    let setup = setup.join(";\n");

    let mut removed = vec![factory.span];
    for decoder in &decoders {
        removed.push(decoder.span);
    }
    if let Some(rotator) = rotator {
        removed.push(rotator);
    }

    if decoders.is_empty() || !factory_references_confined(&semantic, factory.symbol, &removed) {
        let code = apply(text, edits)?;
        return Ok(Outcome {
            code,
            decoded: 0,
            inlined,
        });
    }

    let (calls, mut unresolved, safe_to_remove) =
        collect_calls(&semantic, text, &decoders, factory.symbol, &removed);
    let decoded = if calls.is_empty() {
        Vec::new()
    } else {
        let decode_start = perf.start_stage();
        let values = execute(&setup, &calls)?;
        perf.record("deobfuscate_execute", decode_start);
        values
    };

    let mut replaced = 0;
    for ((span, _), value) in calls.iter().zip(decoded.iter()) {
        let text = match value {
            serde_json::Value::String(value) => quote(value),
            serde_json::Value::Null => "void 0".to_string(),
            _ => {
                unresolved += 1;
                continue;
            }
        };
        edits.push(Edit { span: *span, text });
        replaced += 1;
    }
    if unresolved == 0 && replaced == calls.len() && safe_to_remove {
        for span in removed {
            edits.push(Edit {
                span,
                text: String::new(),
            });
        }
    }
    let code = apply(text, edits)?;
    Ok(Outcome {
        code,
        decoded: replaced,
        inlined,
    })
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn slice(text: &str, span: Span) -> Result<String> {
    text.get(span.start as usize..span.end as usize)
        .map(str::to_string)
        .ok_or_else(|| Error::new("INTERNAL_ERROR", "deobfuscate span out of range"))
}

fn apply(text: &str, mut edits: Vec<Edit>) -> Result<String> {
    edits.sort_by_key(|edit| (edit.span.start, edit.span.end));
    let mut accepted: Vec<Edit> = Vec::with_capacity(edits.len());
    let mut last_end = 0;
    for edit in edits {
        if edit.span.start < last_end {
            continue;
        }
        last_end = edit.span.end;
        accepted.push(edit);
    }
    let mut code = text.to_string();
    for edit in accepted.into_iter().rev() {
        let start = edit.span.start as usize;
        let end = edit.span.end as usize;
        if code.get(start..end).is_none() {
            return Err(Error::new(
                "INTERNAL_ERROR",
                "deobfuscate edit is not a UTF-8 boundary",
            ));
        }
        code.replace_range(start..end, &edit.text);
    }
    Ok(code)
}

fn binding_symbol(pattern: &BindingPattern) -> Option<SymbolId> {
    match pattern {
        BindingPattern::BindingIdentifier(id) => id.symbol_id.get(),
        _ => None,
    }
}

fn reference_symbol(semantic: &Semantic, id: &IdentifierReference) -> Option<SymbolId> {
    id.reference_id
        .get()
        .and_then(|reference| semantic.scoping().get_reference(reference).symbol_id())
}

fn numeric(expr: &Expression) -> Option<f64> {
    match expr {
        Expression::NumericLiteral(literal) => Some(literal.value),
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::UnaryNegation => {
            numeric(&unary.argument).map(|value| -value)
        }
        _ => None,
    }
}

/// Literal array values; `None` is an explicit `undefined` slot.
fn array_values(expr: &Expression) -> Option<Vec<Option<String>>> {
    let Expression::ArrayExpression(array) = expr else {
        return None;
    };
    let mut values = Vec::with_capacity(array.elements.len());
    for element in &array.elements {
        match element {
            ArrayExpressionElement::StringLiteral(literal) => {
                values.push(Some(literal.value.to_string()));
            }
            ArrayExpressionElement::Identifier(id) if id.name == "undefined" => values.push(None),
            ArrayExpressionElement::UnaryExpression(unary)
                if unary.operator == UnaryOperator::Void
                    && numeric(&unary.argument) == Some(0.0) =>
            {
                values.push(None)
            }
            _ => return None,
        }
    }
    Some(values)
}

fn inline_simple_arrays(semantic: &Semantic, edits: &mut Vec<Edit>) -> usize {
    let mut inlined = 0;
    for node in semantic.nodes().iter() {
        let AstKind::VariableDeclarator(declarator) = node.kind() else {
            continue;
        };
        let Some(symbol) = binding_symbol(&declarator.id) else {
            continue;
        };
        let Some(values) = declarator.init.as_ref().and_then(array_values) else {
            continue;
        };
        let AstKind::VariableDeclaration(declaration) = semantic.nodes().parent_kind(node.id())
        else {
            continue;
        };
        if declaration.declarations.len() != 1 {
            continue;
        }
        let mut replacements = Vec::new();
        let mut valid = true;
        for reference in semantic.scoping().get_resolved_references(symbol) {
            if reference.is_write() {
                valid = false;
                break;
            }
            let reference_node = semantic.nodes().get_node(reference.node_id());
            if !matches!(reference_node.kind(), AstKind::IdentifierReference(_)) {
                valid = false;
                break;
            }
            let AstKind::ComputedMemberExpression(member) =
                semantic.nodes().parent_kind(reference_node.id())
            else {
                valid = false;
                break;
            };
            if !matches!(member.object, Expression::Identifier(_)) {
                valid = false;
                break;
            }
            let member_id = semantic.nodes().parent_id(reference_node.id());
            let member_parent = semantic.nodes().parent_kind(member_id);
            if matches!(
                member_parent,
                AstKind::AssignmentExpression(_) | AstKind::UpdateExpression(_)
            ) || matches!(
                member_parent,
                AstKind::UnaryExpression(unary) if unary.operator == UnaryOperator::Delete
            ) {
                valid = false;
                break;
            }
            let Some(index) = numeric(&member.expression) else {
                valid = false;
                break;
            };
            if index < 0.0 || index.fract() != 0.0 {
                valid = false;
                break;
            }
            let Some(Some(value)) = values.get(index as usize) else {
                valid = false;
                break;
            };
            replacements.push(Edit {
                span: member.span,
                text: quote(value),
            });
        }
        if !valid || replacements.is_empty() {
            continue;
        }
        inlined += replacements.len();
        edits.extend(replacements);
        edits.push(Edit {
            span: declaration.span,
            text: String::new(),
        });
    }
    inlined
}

fn function_binding(
    semantic: &Semantic,
    node_id: oxc_semantic::NodeId,
    function: &Function,
) -> Option<(SymbolId, Span)> {
    if function.is_declaration() {
        let id = function.id.as_ref()?;
        return Some((id.symbol_id.get()?, function.span));
    }
    let AstKind::VariableDeclarator(declarator) = semantic.nodes().parent_kind(node_id) else {
        return None;
    };
    let symbol = binding_symbol(&declarator.id)?;
    let declaration = semantic
        .nodes()
        .parent_kind(semantic.nodes().parent_id(node_id));
    let AstKind::VariableDeclaration(declaration) = declaration else {
        return None;
    };
    if declaration.declarations.len() != 1 {
        return None;
    }
    Some((symbol, declaration.span))
}

struct Factory {
    symbol: SymbolId,
    span: Span,
}

fn factory_values(function: &Function, name: &str) -> Option<(String, Vec<Option<String>>)> {
    let body = function.body.as_ref()?;
    let statements = &body.statements;
    if statements.len() < 2 || statements.len() > 3 {
        return None;
    }
    let Statement::VariableDeclaration(declaration) = &statements[0] else {
        return None;
    };
    if declaration.declarations.len() != 1 {
        return None;
    }
    let declarator = &declaration.declarations[0];
    let BindingPattern::BindingIdentifier(array_id) = &declarator.id else {
        return None;
    };
    let values = array_values(declarator.init.as_ref()?)?;
    let array_name = array_id.name.to_string();

    let returns_array = |function: &Function<'_>| {
        let Some(body) = &function.body else {
            return false;
        };
        let [Statement::ReturnStatement(ret)] = body.statements.as_slice() else {
            return false;
        };
        matches!(ret.argument.as_ref(), Some(Expression::Identifier(id)) if id.name.as_str() == array_name)
    };

    let assignment_ok = |statement: &Statement<'_>| {
        let expression = match statement {
            Statement::ReturnStatement(ret) => ret.argument.as_ref()?,
            Statement::ExpressionStatement(stmt) => &stmt.expression,
            _ => return None,
        };
        let assign = match expression {
            Expression::AssignmentExpression(assign) => assign,
            Expression::CallExpression(call) => match &call.callee {
                Expression::AssignmentExpression(assign) => assign,
                _ => return None,
            },
            _ => return None,
        };
        if assign.operator != AssignmentOperator::Assign {
            return None;
        }
        let AssignmentTarget::AssignmentTargetIdentifier(left) = &assign.left else {
            return None;
        };
        if left.name.as_str() != name {
            return None;
        }
        let Expression::FunctionExpression(function) = &assign.right else {
            return None;
        };
        returns_array(function).then_some(())
    };

    match statements.as_slice() {
        [_, second] if assignment_ok(second).is_some() => Some((array_name, values)),
        [_, second, third] => {
            assignment_ok(second)?;
            let Statement::ReturnStatement(ret) = third else {
                return None;
            };
            let Some(Expression::CallExpression(call)) = ret.argument.as_ref() else {
                return None;
            };
            let Expression::Identifier(callee) = &call.callee else {
                return None;
            };
            (callee.name.as_str() == name && call.arguments.is_empty())
                .then_some((array_name, values))
        }
        _ => None,
    }
}

fn find_string_array(semantic: &Semantic) -> Option<Factory> {
    for node in semantic.nodes().iter() {
        let AstKind::Function(function) = node.kind() else {
            continue;
        };
        let Some((symbol, span)) = function_binding(semantic, node.id(), function) else {
            continue;
        };
        let name = semantic.scoping().symbol_name(symbol);
        if factory_values(function, name).is_none() {
            continue;
        }
        return Some(Factory { symbol, span });
    }
    None
}

fn is_call_expression(expression: &Expression) -> bool {
    match expression {
        Expression::CallExpression(_) => true,
        Expression::UnaryExpression(unary) => is_call_expression(&unary.argument),
        Expression::ParenthesizedExpression(paren) => is_call_expression(&paren.expression),
        _ => false,
    }
}

fn factory_references_confined(semantic: &Semantic, factory: SymbolId, spans: &[Span]) -> bool {
    semantic
        .scoping()
        .get_resolved_references(factory)
        .all(|reference| {
            let span = semantic.nodes().get_node(reference.node_id()).span();
            spans.iter().any(|outer| within(span, *outer))
        })
}

fn find_rotator(semantic: &Semantic, text: &str, factory: SymbolId) -> Option<Span> {
    for reference in semantic.scoping().get_resolved_references(factory) {
        let mut result = None;
        for ancestor in semantic.nodes().ancestors(reference.node_id()) {
            let AstKind::ExpressionStatement(statement) = ancestor.kind() else {
                continue;
            };
            if !is_call_expression(&statement.expression) {
                continue;
            }
            let Some(source) = text.get(statement.span.start as usize..statement.span.end as usize)
            else {
                continue;
            };
            if source.contains("push") && source.contains("shift") {
                result = Some(statement.span);
            }
        }
        if result.is_some() {
            return result;
        }
    }
    None
}

struct Decoder {
    symbol: SymbolId,
    span: Span,
}

fn within(span: Span, outer: Span) -> bool {
    span.start >= outer.start && span.end <= outer.end
}

fn body_calls(semantic: &Semantic, body: Span, symbol: SymbolId) -> bool {
    semantic.nodes().iter().any(|node| {
        let AstKind::CallExpression(call) = node.kind() else {
            return false;
        };
        if !within(call.span, body) {
            return false;
        }
        let Expression::Identifier(callee) = &call.callee else {
            return false;
        };
        reference_symbol(semantic, callee) == Some(symbol)
    })
}

fn body_has_computed_member(semantic: &Semantic, body: Span) -> bool {
    semantic.nodes().iter().any(|node| {
        matches!(
            node.kind(),
            AstKind::ComputedMemberExpression(member)
                if within(member.span, body) && matches!(member.object, Expression::Identifier(_))
        )
    })
}

fn find_decoders(semantic: &Semantic, factory: SymbolId) -> Vec<Decoder> {
    let mut decoders = Vec::new();
    for node in semantic.nodes().iter() {
        let AstKind::Function(function) = node.kind() else {
            continue;
        };
        let Some(body) = &function.body else {
            continue;
        };
        let Some((symbol, span)) = function_binding(semantic, node.id(), function) else {
            continue;
        };
        if !body_calls(semantic, body.span, factory)
            || !body_has_computed_member(semantic, body.span)
        {
            continue;
        }
        decoders.push(Decoder { symbol, span });
    }
    decoders.sort_by_key(|decoder| decoder.span.start);
    decoders
}

fn collect_calls(
    semantic: &Semantic,
    text: &str,
    decoders: &[Decoder],
    factory: SymbolId,
    removed: &[Span],
) -> (Vec<(Span, String)>, usize, bool) {
    let mut unresolved = 0;
    let mut names: std::collections::HashMap<SymbolId, String> = decoders
        .iter()
        .map(|decoder| {
            (
                decoder.symbol,
                semantic.scoping().symbol_name(decoder.symbol).to_string(),
            )
        })
        .collect();
    loop {
        let before = names.len();
        for node in semantic.nodes().iter() {
            let AstKind::VariableDeclarator(declarator) = node.kind() else {
                continue;
            };
            let Some(init) = &declarator.init else {
                continue;
            };
            let Expression::Identifier(id) = init else {
                continue;
            };
            let Some(symbol) = reference_symbol(semantic, id) else {
                continue;
            };
            if let Some(name) = names.get(&symbol).cloned()
                && let Some(alias) = binding_symbol(&declarator.id)
            {
                names.entry(alias).or_insert(name);
            }
        }
        if names.len() == before {
            break;
        }
    }

    let mut calls = Vec::new();
    for node in semantic.nodes().iter() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        if removed.iter().any(|span| within(call.span, *span)) {
            continue;
        }
        let Expression::Identifier(callee) = &call.callee else {
            continue;
        };
        let Some(symbol) = reference_symbol(semantic, callee) else {
            continue;
        };
        let Some(decoder_name) = names.get(&symbol) else {
            continue;
        };
        let mut args = Vec::with_capacity(call.arguments.len());
        let mut valid = true;
        for argument in &call.arguments {
            let expression = argument.to_expression();
            let raw = if numeric(expression).is_some()
                || matches!(expression, Expression::StringLiteral(_))
            {
                text.get(expression.span().start as usize..expression.span().end as usize)
                    .map(str::to_string)
            } else if let Expression::Identifier(id) = expression {
                reference_symbol(semantic, id)
                    .and_then(|symbol| literal_binding(semantic, text, symbol))
            } else {
                None
            };
            let Some(raw) = raw else {
                valid = false;
                break;
            };
            args.push(raw.to_string());
        }
        if !valid || args.is_empty() {
            unresolved += 1;
            continue;
        }
        calls.push((call.span, format!("{decoder_name}({})", args.join(","))));
    }
    calls.sort_by_key(|(span, _)| span.start);
    calls.dedup_by_key(|(span, _)| span.start);

    let kept: Vec<Span> = calls.iter().map(|(span, _)| *span).collect();
    let mut safe = unresolved == 0;
    for symbol in names.keys().copied().chain(std::iter::once(factory)) {
        for reference in semantic.scoping().get_resolved_references(symbol) {
            let span = semantic.nodes().get_node(reference.node_id()).span();
            if removed.iter().any(|outer| within(span, *outer))
                || kept.iter().any(|outer| within(span, *outer))
            {
                continue;
            }
            safe = false;
        }
    }
    (calls, unresolved, safe)
}

fn literal_binding(semantic: &Semantic, text: &str, symbol: SymbolId) -> Option<String> {
    let literal = |expression: &Expression| {
        (numeric(expression).is_some() || matches!(expression, Expression::StringLiteral(_)))
            .then(|| {
                text.get(expression.span().start as usize..expression.span().end as usize)
                    .map(str::to_string)
            })
            .flatten()
    };
    let mut assigned = None;
    let mut writes = 0;
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::VariableDeclarator(declarator)
                if binding_symbol(&declarator.id) == Some(symbol) =>
            {
                if let Some(value) = declarator.init.as_ref().and_then(&literal) {
                    return Some(value);
                }
            }
            AstKind::AssignmentExpression(assignment)
                if assignment.operator == AssignmentOperator::Assign =>
            {
                let AssignmentTarget::AssignmentTargetIdentifier(target) = &assignment.left else {
                    continue;
                };
                if reference_symbol(semantic, target) != Some(symbol) {
                    continue;
                }
                writes += 1;
                assigned = literal(&assignment.right);
            }
            _ => {}
        }
    }
    (writes == 1).then_some(assigned).flatten()
}

fn execute(setup: &str, calls: &[(Span, String)]) -> Result<Vec<serde_json::Value>> {
    let body = calls
        .iter()
        .map(|(_, call)| call.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let code = format!(
        "(function(){{try{{\n{setup}\nreturn JSON.stringify([{body}]);\n}}catch(e){{return 'ERR:'+String(e);}}}})()"
    );

    let runtime = rquickjs::Runtime::new()
        .map_err(|error| Error::new("DECOMPILE_ERROR", format!("decoder engine: {error}")))?;
    runtime.set_memory_limit(64 * 1024 * 1024);
    runtime.set_max_stack_size(512 * 1024);
    let deadline = Instant::now() + Duration::from_secs(2);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = rquickjs::Context::full(&runtime)
        .map_err(|error| Error::new("DECOMPILE_ERROR", format!("decoder context: {error}")))?;
    let result = context.with(|ctx| ctx.eval::<String, _>(code));
    let json = result.map_err(|error| {
        let message = error.to_string();
        if message.contains("interrupted") {
            Error::new("RESOURCE_LIMIT", "decoder execution timed out")
        } else {
            Error::new("DECOMPILE_ERROR", format!("decoder execution: {message}"))
        }
    })?;
    if let Some(message) = json.strip_prefix("ERR:") {
        return Err(Error::new(
            "DECOMPILE_ERROR",
            format!("decoder execution: {message}"),
        ));
    }
    serde_json::from_str(&json)
        .map_err(|error| Error::new("DECOMPILE_ERROR", format!("decoder output: {error}")))
}
