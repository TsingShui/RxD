//! Recognized literal-table decoders: a string array, an index function, a rotation.
//! Bounded static recovery only; unrecognized or ambiguous shapes stay unresolved.

use std::collections::{HashMap, HashSet};

use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, UnaryOperator};

use crate::strings::JsString;

/// A recognized rotation checksum indexes only a few elements; larger shapes are not claimed.
const MAX_CHECKSUM_INDICES: usize = 64;
/// Identifier indirection stays short in generated code; longer chains are not followed.
const MAX_FOLLOW: usize = 8;

pub struct Decoders {
    tables: Vec<Table>,
    /// Every identifier proven to index a recognized table, including aliases.
    names: HashMap<String, usize>,
    /// Table, decoder and rotation syntax never carries a reportable lookup value.
    suppressed: Vec<Span>,
}

struct Table {
    offset: i64,
    values: Vec<JsString>,
}

pub struct Resolved<'a> {
    pub value: &'a JsString,
}

impl Decoders {
    pub fn empty() -> Self {
        Self {
            tables: Vec::new(),
            names: HashMap::new(),
            suppressed: Vec::new(),
        }
    }

    /// Recognize whole tables only: one failed element or an ambiguous rotation rejects it.
    pub fn build(program: &Program<'_>) -> Self {
        let mut scan = Scan::default();
        scan.visit_program(program);
        let recognized = recognize(&scan);
        let mut decoders = Self {
            suppressed: recognized.suppressed,
            ..Self::empty()
        };
        for (names, offset, values) in recognized.tables {
            let index = decoders.tables.len();
            for name in names {
                decoders.names.insert(name, index);
            }
            decoders.tables.push(Table { offset, values });
        }
        decoders
    }

    /// Resolve `decoder(<numeric index>)`; anything else is not a table lookup.
    pub fn call(&self, call: &CallExpression<'_>) -> Option<Resolved<'_>> {
        let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
            return None;
        };
        let table = &self.tables[*self.names.get(callee.name.as_str())?];
        if call.arguments.len() != 1 {
            return None;
        }
        let index = argument_index(&call.arguments[0])?.checked_sub(table.offset)?;
        let value = table.values.get(usize::try_from(index).ok()?)?;
        Some(Resolved { value })
    }

    /// Resolve a value expression: a decoder call, or an optional-chained one.
    pub fn expression(&self, expression: &Expression<'_>) -> Option<Resolved<'_>> {
        match expression.get_inner_expression() {
            Expression::CallExpression(call) => self.call(call),
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => self.call(call),
                _ => None,
            },
            _ => None,
        }
    }

    /// Table, decoder and rotation syntax proves a value; it is not a lookup site.
    pub fn suppressed(&self, span: Span) -> bool {
        self.suppressed
            .iter()
            .any(|outer| outer.start <= span.start && span.end <= outer.end)
    }
}

fn argument_index(argument: &Argument<'_>) -> Option<i64> {
    match argument {
        Argument::NumericLiteral(number) => Some(number.value as i64),
        Argument::StringLiteral(string) => parse_int_utf8(string.value.as_str()),
        _ => None,
    }
}

/// Integer prefix in one of the two radices generated decoders use.
fn parse_int_utf8(text: &str) -> Option<i64> {
    let text = text.trim_start_matches([' ', '\t', '\r', '\n', '\u{b}', '\u{c}']);
    let (negative, text) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (radix, digits) = match text.get(..2) {
        Some(prefix) if prefix.eq_ignore_ascii_case("0x") => (16, &text[2..]),
        _ => (10, text),
    };
    let end = digits
        .find(|c: char| !c.is_digit(radix))
        .unwrap_or(digits.len());
    if end == 0 {
        return None;
    }
    let value = i64::from_str_radix(&digits[..end], radix).ok()?;
    Some(if negative { -value } else { value })
}

struct Frame<'a> {
    name: Option<String>,
    params: Vec<String>,
    span: Span,
    arrays: Vec<&'a ArrayExpression<'a>>,
    calls: Vec<&'a CallExpression<'a>>,
    declarators: Vec<&'a VariableDeclarator<'a>>,
    returns: Vec<&'a ReturnStatement<'a>>,
    assignments: Vec<&'a AssignmentExpression<'a>>,
    ifs: Vec<&'a IfStatement<'a>>,
}

impl<'a> Frame<'a> {
    fn init(&self, name: &str) -> Option<&'a Expression<'a>> {
        self.declarators.iter().find_map(|declarator| {
            let id = declarator.id.get_binding_identifier()?;
            (id.name.as_str() == name)
                .then_some(declarator.init.as_ref())
                .flatten()
        })
    }
}

#[derive(Default)]
struct Scan<'a> {
    frames: Vec<Frame<'a>>,
    /// Completed frames; nesting order does not matter after the walk.
    all: Vec<Frame<'a>>,
    bindings: HashMap<String, usize>,
    calls: Vec<&'a CallExpression<'a>>,
}

impl<'a> Scan<'a> {
    /// A name bound once in the file cannot be shadowed elsewhere.
    fn unique(&self, name: &str) -> bool {
        self.bindings.get(name).copied() == Some(1)
    }
}

impl<'a> Visit<'a> for Scan<'a> {
    fn enter_node(&mut self, node: AstKind<'a>) {
        match node {
            // The program frame holds file-level aliases no function encloses.
            AstKind::Program(program) => self.frames.push(Frame {
                name: None,
                params: Vec::new(),
                span: program.span,
                arrays: Vec::new(),
                calls: Vec::new(),
                declarators: Vec::new(),
                returns: Vec::new(),
                assignments: Vec::new(),
                ifs: Vec::new(),
            }),
            AstKind::Function(function) => {
                let params = &function.params;
                self.frames.push(Frame {
                    name: function.id.as_ref().map(|id| id.name.as_str().to_owned()),
                    params: parameter_names(params),
                    span: function.span,
                    arrays: Vec::new(),
                    calls: Vec::new(),
                    declarators: Vec::new(),
                    returns: Vec::new(),
                    assignments: Vec::new(),
                    ifs: Vec::new(),
                });
            }
            AstKind::ArrowFunctionExpression(arrow) => {
                let params = &arrow.params;
                self.frames.push(Frame {
                    name: None,
                    params: parameter_names(params),
                    span: arrow.span,
                    arrays: Vec::new(),
                    calls: Vec::new(),
                    declarators: Vec::new(),
                    returns: Vec::new(),
                    assignments: Vec::new(),
                    ifs: Vec::new(),
                });
            }
            AstKind::BindingIdentifier(id) => {
                *self
                    .bindings
                    .entry(id.name.as_str().to_owned())
                    .or_default() += 1;
            }
            _ => {}
        }
        if let AstKind::CallExpression(call) = node {
            self.calls.push(call);
        }
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        match node {
            AstKind::ArrayExpression(array) => frame.arrays.push(array),
            AstKind::CallExpression(call) => frame.calls.push(call),
            AstKind::VariableDeclarator(declarator) => frame.declarators.push(declarator),
            AstKind::ReturnStatement(ret) => frame.returns.push(ret),
            AstKind::AssignmentExpression(assignment) => frame.assignments.push(assignment),
            AstKind::IfStatement(statement) => frame.ifs.push(statement),
            _ => {}
        }
    }

    fn leave_node(&mut self, node: AstKind<'a>) {
        if matches!(
            node,
            AstKind::Program(_) | AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
        ) && let Some(frame) = self.frames.pop()
        {
            self.all.push(frame);
        }
    }
}

fn parameter_names(params: &FormalParameters<'_>) -> Vec<String> {
    params
        .items
        .iter()
        .filter_map(|parameter| {
            parameter
                .pattern
                .get_binding_identifier()
                .map(|id| id.name.as_str().to_owned())
        })
        .collect()
}

struct Recognized {
    /// Every proven name for one table, its index shift, and its values.
    tables: Vec<(Vec<String>, i64, Vec<JsString>)>,
    suppressed: Vec<Span>,
}

fn recognize(scan: &Scan<'_>) -> Recognized {
    let mut recognized = Recognized {
        tables: Vec::new(),
        suppressed: Vec::new(),
    };
    // Literal tables: a zero-argument function that builds one array of strings.
    let mut factories: Vec<(&str, &ArrayExpression, Span)> = Vec::new();
    for frame in &scan.all {
        let Some(name) = frame.name.as_deref() else {
            continue;
        };
        if !frame.params.is_empty() || !scan.unique(name) {
            continue;
        }
        let Some(array) = frame.arrays.iter().copied().find(|a| is_string_table(a)) else {
            continue;
        };
        factories.push((name, array, frame.span));
    }
    // Decoders: a function that returns `table[...]` indexed by its first parameter.
    let mut decoders: Vec<(&str, usize, i64, Span)> = Vec::new();
    for frame in &scan.all {
        let (Some(name), Some(parameter)) = (frame.name.as_deref(), frame.params.first()) else {
            continue;
        };
        if !scan.unique(name) {
            continue;
        }
        let Some((factory, offset)) = decoder_shape(frame, parameter, &factories) else {
            continue;
        };
        decoders.push((name, factory, offset, frame.span));
    }
    if decoders.is_empty() {
        return recognized;
    }
    let mut targets: HashMap<String, usize> = decoders
        .iter()
        .enumerate()
        .map(|(index, (name, _, _, _))| ((*name).to_owned(), index))
        .collect();
    // Aliases name the same table, so `const a = d; a(0x10)` stays exact.
    for _ in 0..MAX_FOLLOW {
        let mut added = false;
        for frame in &scan.all {
            for declarator in &frame.declarators {
                let (Some(id), Some(Expression::Identifier(init))) = (
                    declarator.id.get_binding_identifier(),
                    declarator.init.as_ref(),
                ) else {
                    continue;
                };
                let name = id.name.as_str();
                if targets.contains_key(name) || !scan.unique(name) {
                    continue;
                }
                if let Some(target) = targets.get(init.name.as_str()).copied() {
                    targets.insert(name.to_owned(), target);
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
    }
    let mut solved: HashMap<usize, Vec<JsString>> = HashMap::new();
    for (index, (factory, array, span)) in factories.iter().enumerate() {
        let Some(values) = string_values(array) else {
            continue;
        };
        // Checksums call the decoder through its own names, including local aliases.
        let mut names: HashMap<String, i64> = HashMap::new();
        for (target, source) in &targets {
            if let Some((_, candidate, offset, _)) = decoders.get(*source)
                && *candidate == index
            {
                names.insert(target.clone(), *offset);
            }
        }
        let Some(rotated) = rotate(scan, factory, &values, &names, &mut recognized) else {
            continue;
        };
        solved.insert(index, rotated);
        recognized.suppressed.push(*span);
    }
    // Whole-table only: a table is claimed when every element decoded and rotation is unique.
    for (index, (_, factory, offset, span)) in decoders.iter().enumerate() {
        let Some(values) = solved.get(factory) else {
            continue;
        };
        let mut names: Vec<String> = targets
            .iter()
            .filter(|(_, target)| **target == index)
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        recognized.tables.push((names, *offset, values.clone()));
        recognized.suppressed.push(*span);
    }
    recognized
}

fn decoder_shape<'a>(
    frame: &Frame<'a>,
    parameter: &str,
    factories: &[(&str, &'a ArrayExpression<'a>, Span)],
) -> Option<(usize, i64)> {
    let factory = frame.calls.iter().find_map(|call| {
        let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
            return None;
        };
        let name = callee.name.as_str();
        if !is_call_to(call, name) {
            return None;
        }
        factories
            .iter()
            .position(|(factory, _, _)| *factory == name)
    })?;
    let member = frame
        .returns
        .iter()
        .rev()
        .find_map(|ret| follow_member(frame, ret.argument.as_ref()?, 0))?;
    if !is_table_source(frame, &member.object, factories[factory].0) {
        return None;
    }
    let offset = index_offset(frame, parameter, &member.expression)?;
    Some((factory, offset))
}

/// Follow local declarations down to the indexed member a decoder returns.
fn follow_member<'a>(
    frame: &Frame<'a>,
    expression: &'a Expression<'a>,
    depth: usize,
) -> Option<&'a ComputedMemberExpression<'a>> {
    if depth > MAX_FOLLOW {
        return None;
    }
    match expression.get_inner_expression() {
        Expression::ComputedMemberExpression(member) => Some(member),
        Expression::Identifier(id) => {
            follow_member(frame, frame.init(id.name.as_str())?, depth + 1)
        }
        _ => None,
    }
}

fn is_table_source(frame: &Frame<'_>, object: &Expression<'_>, factory: &str) -> bool {
    match object.get_inner_expression() {
        Expression::Identifier(id) => {
            if id.name.as_str() == factory {
                return true;
            }
            matches!(
                frame.init(id.name.as_str()).map(Expression::get_inner_expression),
                Some(Expression::CallExpression(call)) if is_call_to(call, factory)
            )
        }
        Expression::CallExpression(call) => is_call_to(call, factory),
        _ => false,
    }
}

fn is_call_to(call: &CallExpression<'_>, name: &str) -> bool {
    call.arguments.is_empty()
        && matches!(
            call.callee.get_inner_expression(),
            Expression::Identifier(id) if id.name.as_str() == name
        )
}

/// `table[index]` where the index is the first parameter, shifted by one constant.
fn index_offset(frame: &Frame<'_>, parameter: &str, key: &Expression<'_>) -> Option<i64> {
    match key.get_inner_expression() {
        Expression::Identifier(id) if id.name.as_str() == parameter => {
            Some(shift_of_parameter(frame, parameter).unwrap_or(0))
        }
        Expression::BinaryExpression(binary) => {
            let Expression::Identifier(left) = binary.left.get_inner_expression() else {
                return None;
            };
            if left.name.as_str() != parameter {
                return None;
            }
            let Expression::NumericLiteral(right) = binary.right.get_inner_expression() else {
                return None;
            };
            let value = right.value as i64;
            match binary.operator {
                BinaryOperator::Subtraction => Some(value),
                BinaryOperator::Addition => Some(-value),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `parameter = parameter - K` or `parameter -= K` rewrites the index before lookup.
fn shift_of_parameter(frame: &Frame<'_>, parameter: &str) -> Option<i64> {
    frame.assignments.iter().find_map(|assignment| {
        let target = match &assignment.left {
            AssignmentTarget::AssignmentTargetIdentifier(id) => id.name.as_str(),
            _ => return None,
        };
        if target != parameter {
            return None;
        }
        match assignment.operator {
            AssignmentOperator::Assign => {
                let Expression::BinaryExpression(binary) = assignment.right.get_inner_expression()
                else {
                    return None;
                };
                let Expression::Identifier(from) = binary.left.get_inner_expression() else {
                    return None;
                };
                if from.name.as_str() != parameter {
                    return None;
                }
                let Expression::NumericLiteral(number) = binary.right.get_inner_expression() else {
                    return None;
                };
                match binary.operator {
                    BinaryOperator::Subtraction => Some(number.value as i64),
                    BinaryOperator::Addition => Some(-(number.value as i64)),
                    _ => None,
                }
            }
            AssignmentOperator::Subtraction | AssignmentOperator::Addition => {
                let Expression::NumericLiteral(number) = assignment.right.get_inner_expression()
                else {
                    return None;
                };
                let value = number.value as i64;
                Some(if assignment.operator == AssignmentOperator::Subtraction {
                    value
                } else {
                    -value
                })
            }
            _ => None,
        }
    })
}

fn is_string_table(array: &ArrayExpression<'_>) -> bool {
    array.elements.len() >= 2
        && array
            .elements
            .iter()
            .all(|element| matches!(element, ArrayExpressionElement::StringLiteral(_)))
}

fn string_values(array: &ArrayExpression<'_>) -> Option<Vec<JsString>> {
    array
        .elements
        .iter()
        .map(|element| match element {
            ArrayExpressionElement::StringLiteral(string) => Some(JsString::decoded(
                string.value.as_str(),
                string.lone_surrogates,
            )),
            _ => None,
        })
        .collect()
}

/// Solve `while (checksum !== constant) array.push(array.shift())` by trying every offset.
fn rotate(
    scan: &Scan<'_>,
    factory: &str,
    values: &[JsString],
    names: &HashMap<String, i64>,
    recognized: &mut Recognized,
) -> Option<Vec<JsString>> {
    for call in &scan.calls {
        let Some(first) = call.arguments.first() else {
            continue;
        };
        let Argument::Identifier(source) = first else {
            continue;
        };
        if source.name.as_str() != factory {
            continue;
        }
        let Some(frame) = scan
            .all
            .iter()
            .find(|frame| frame.span == call.callee.get_inner_expression().span())
        else {
            continue;
        };
        if frame.params.len() != 2 {
            continue;
        }
        let (Some(target), Some(checksum)) = (rotation_target(call), checksum(frame)) else {
            continue;
        };
        let mut indices = HashSet::new();
        if !collect_indices(checksum, names, &mut indices) || indices.len() > MAX_CHECKSUM_INDICES {
            continue;
        }
        let length = values.len();
        let eval = Eval {
            frame,
            values,
            names,
        };
        let matches: Vec<usize> = (0..length)
            .filter(|shift| eval.number(checksum, *shift).is_some_and(|v| v == target))
            .collect();
        let [shift] = matches[..] else {
            continue;
        };
        recognized.suppressed.push(call.callee.span());
        return Some(
            (0..length)
                .map(|index| values[(index + shift) % length].clone())
                .collect(),
        );
    }
    (!has_rotation(scan, factory)).then(|| values.to_vec())
}

fn has_rotation(scan: &Scan<'_>, factory: &str) -> bool {
    scan.calls.iter().any(|call| {
        matches!(call.arguments.first(), Some(Argument::Identifier(id)) if id.name.as_str() == factory)
            && matches!(
                call.callee.get_inner_expression(),
                Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_)
            )
    })
}

fn rotation_target(call: &CallExpression<'_>) -> Option<f64> {
    match call.arguments.get(1)? {
        Argument::NumericLiteral(number) => Some(number.value),
        _ => None,
    }
}

/// `if (checksum === parameter) break` names the expression the rotation must satisfy.
fn checksum<'a>(frame: &'a Frame<'a>) -> Option<&'a Expression<'a>> {
    let parameter = frame.params.get(1)?;
    frame.ifs.iter().find_map(|statement| {
        let Expression::BinaryExpression(binary) = statement.test.get_inner_expression() else {
            return None;
        };
        match binary.operator {
            BinaryOperator::StrictEquality
            | BinaryOperator::StrictInequality
            | BinaryOperator::Equality
            | BinaryOperator::Inequality => {}
            _ => return None,
        }
        match (
            binary.right.get_inner_expression(),
            binary.left.get_inner_expression(),
        ) {
            (Expression::Identifier(id), checksum) if id.name.as_str() == parameter => {
                Some(checksum)
            }
            _ => None,
        }
    })
}

fn collect_indices(
    expression: &Expression<'_>,
    names: &HashMap<String, i64>,
    indices: &mut HashSet<i64>,
) -> bool {
    match expression.get_inner_expression() {
        Expression::CallExpression(call) => {
            let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
                return true;
            };
            if !names.contains_key(callee.name.as_str()) {
                return true;
            }
            match call.arguments.first() {
                Some(argument) => match argument_index(argument) {
                    Some(index) => indices.insert(index),
                    None => return false,
                },
                None => return false,
            };
            true
        }
        Expression::BinaryExpression(binary) => {
            collect_indices(&binary.left, names, indices)
                && collect_indices(&binary.right, names, indices)
        }
        Expression::UnaryExpression(unary) => collect_indices(&unary.argument, names, indices),
        Expression::ParenthesizedExpression(paren) => {
            collect_indices(&paren.expression, names, indices)
        }
        _ => true,
    }
}

/// Bounded numeric evaluation of a rotation checksum: no host access, no user calls.
struct Eval<'a, 'b> {
    frame: &'b Frame<'a>,
    values: &'b [JsString],
    names: &'b HashMap<String, i64>,
}

enum Number<'b> {
    Value(f64),
    Text(&'b JsString),
}

impl<'a, 'b> Eval<'a, 'b> {
    fn number(&self, expression: &'a Expression<'a>, shift: usize) -> Option<f64> {
        match self.value(expression, shift, 0)? {
            Number::Value(number) => Some(number),
            Number::Text(text) => Some(to_number(text)),
        }
    }

    fn value(
        &self,
        expression: &'a Expression<'a>,
        shift: usize,
        depth: usize,
    ) -> Option<Number<'b>> {
        if depth > MAX_FOLLOW * 4 {
            return None;
        }
        let next = depth + 1;
        match expression.get_inner_expression() {
            Expression::NumericLiteral(number) => Some(Number::Value(number.value)),
            Expression::Identifier(id) => {
                self.value(self.frame.init(id.name.as_str())?, shift, next)
            }
            Expression::UnaryExpression(unary) => {
                let value = self.number(&unary.argument, shift)?;
                match unary.operator {
                    UnaryOperator::UnaryNegation => Some(Number::Value(-value)),
                    UnaryOperator::UnaryPlus => Some(Number::Value(value)),
                    _ => None,
                }
            }
            Expression::BinaryExpression(binary) => {
                let left = self.number(&binary.left, shift)?;
                let right = self.number(&binary.right, shift)?;
                let value = match binary.operator {
                    BinaryOperator::Addition => left + right,
                    BinaryOperator::Subtraction => left - right,
                    BinaryOperator::Multiplication => left * right,
                    BinaryOperator::Division => left / right,
                    BinaryOperator::Remainder => left % right,
                    _ => return None,
                };
                Some(Number::Value(value))
            }
            Expression::ParenthesizedExpression(paren) => {
                self.value(&paren.expression, shift, next)
            }
            Expression::CallExpression(call) => {
                let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
                    return None;
                };
                let name = callee.name.as_str();
                let [argument] = call.arguments.as_slice() else {
                    return None;
                };
                if let Some(offset) = self.names.get(name) {
                    let index = argument_index(argument)?.checked_sub(*offset)?;
                    let length = self.values.len();
                    let index = usize::try_from(index).ok()?;
                    if index >= length {
                        return None;
                    }
                    return Some(Number::Text(&self.values[(index + shift) % length]));
                }
                let inner = match argument {
                    Argument::StringLiteral(string) => {
                        JsString::decoded(string.value.as_str(), string.lone_surrogates)
                    }
                    _ => match self.value(argument.to_expression(), shift, next)? {
                        Number::Text(text) => text.clone(),
                        Number::Value(number) => JsString::from(number.to_string().as_str()),
                    },
                };
                match name {
                    "parseInt" | "Number" => Some(Number::Value(parse_int(&inner))),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// ECMAScript parseInt restricted to the radices a generated checksum uses.
fn parse_int(value: &JsString) -> f64 {
    value
        .to_utf8()
        .and_then(|text| parse_int_utf8(&text))
        .map_or(f64::NAN, |value| value as f64)
}

/// ECMAScript ToNumber for a string, without running Rust's decimal extensions.
fn to_number(value: &JsString) -> f64 {
    let Some(text) = value.to_utf8() else {
        return f64::NAN;
    };
    let text = text.trim_matches([' ', '\t', '\r', '\n', '\u{b}', '\u{c}']);
    if text.is_empty() {
        return 0.0;
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).map_or(f64::NAN, |value| value as f64);
    }
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_digit() || b"+-.eE".contains(&byte))
    {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}
