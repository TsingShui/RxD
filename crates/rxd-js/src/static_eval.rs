//! Bounded evaluation of syntax-local constant expressions.
//!
//! This is intentionally not a JavaScript interpreter: it never executes user-defined
//! functions, reads globals, follows bindings, or performs I/O. It only evaluates a small
//! allowlist of literal expressions and pure string helpers whose arguments are themselves
//! statically known.

use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BinaryExpression, Expression, TemplateLiteral,
};
use oxc_syntax::operator::{BinaryOperator, UnaryOperator};
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
    Array(Vec<Option<Value>>),
}

impl Value {
    fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
            Self::String(value) => value.parse().ok(),
            Self::Null => Some(0.0),
            Self::Array(_) => None,
        }
    }

    /// Borrow the text form without allocating for the common string case.
    fn as_cow(&self) -> Option<Cow<'_, str>> {
        match self {
            Self::String(value) => Some(Cow::Borrowed(value.as_str())),
            Self::Number(value) => {
                if !value.is_finite() {
                    None
                } else if value.fract() == 0.0 {
                    Some(Cow::Owned(format!("{value:.0}")))
                } else {
                    Some(Cow::Owned(value.to_string()))
                }
            }
            Self::Bool(value) => Some(Cow::Borrowed(if *value { "true" } else { "false" })),
            Self::Null => Some(Cow::Borrowed("null")),
            Self::Array(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub depth: usize,
    pub steps: usize,
    pub value_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 32,
            steps: 512,
            value_bytes: 64 * 1024,
        }
    }
}

#[allow(dead_code)]
pub fn evaluate(expression: &Expression<'_>, limits: Limits) -> Option<Value> {
    let mut evaluator = Evaluator { limits, steps: 0 };
    evaluator.expression(expression, 0)
}

pub fn evaluate_binary(binary: &BinaryExpression<'_>, limits: Limits) -> Option<Value> {
    let mut evaluator = Evaluator { limits, steps: 0 };
    evaluator.binary(binary, 0)
}

pub fn evaluate_call(call: &oxc_ast::ast::CallExpression<'_>, limits: Limits) -> Option<Value> {
    let mut evaluator = Evaluator { limits, steps: 0 };
    evaluator.call(call, 0)
}

pub fn evaluate_template(template: &TemplateLiteral<'_>, limits: Limits) -> Option<Value> {
    let mut evaluator = Evaluator { limits, steps: 0 };
    evaluator.template(template, 0)
}

pub fn evaluate_member(
    member: &oxc_ast::ast::ComputedMemberExpression<'_>,
    limits: Limits,
) -> Option<Value> {
    let mut evaluator = Evaluator { limits, steps: 0 };
    evaluator.member(member, 0)
}

struct Evaluator {
    limits: Limits,
    steps: usize,
}

impl Evaluator {
    fn step(&mut self, depth: usize) -> Option<()> {
        if depth > self.limits.depth {
            return None;
        }
        self.steps = self.steps.checked_add(1)?;
        (self.steps <= self.limits.steps).then_some(())
    }

    fn expression(&mut self, expression: &Expression<'_>, depth: usize) -> Option<Value> {
        self.step(depth)?;
        let value = match expression.without_parentheses() {
            Expression::StringLiteral(literal) => Value::String(literal.value.as_str().to_owned()),
            Expression::NumericLiteral(literal) => Value::Number(literal.value),
            Expression::BooleanLiteral(literal) => Value::Bool(literal.value),
            Expression::NullLiteral(_) => Value::Null,
            Expression::TemplateLiteral(template) => self.template(template, depth + 1)?,
            Expression::ArrayExpression(array) => {
                let mut values = Vec::with_capacity(array.elements.len());
                for element in &array.elements {
                    let value = match element {
                        ArrayExpressionElement::Elision(_) => None,
                        ArrayExpressionElement::SpreadElement(_) => return None,
                        element => Some(self.expression(element.as_expression()?, depth + 1)?),
                    };
                    values.push(value);
                }
                Value::Array(values)
            }
            Expression::ComputedMemberExpression(member) => self.member(member, depth + 1)?,
            Expression::UnaryExpression(unary) => {
                let value = self.expression(&unary.argument, depth + 1)?;
                match unary.operator {
                    UnaryOperator::UnaryPlus => Value::Number(value.as_number()?),
                    UnaryOperator::UnaryNegation => Value::Number(-value.as_number()?),
                    UnaryOperator::LogicalNot => Value::Bool(!truthy(&value)),
                    UnaryOperator::BitwiseNot => {
                        Value::Number((!(value.as_number()? as i32)) as f64)
                    }
                    _ => return None,
                }
            }
            Expression::BinaryExpression(binary) => self.binary(binary, depth + 1)?,
            Expression::CallExpression(call) => self.call(call, depth + 1)?,
            _ => return None,
        };
        if let Value::String(value) = &value
            && value.len() > self.limits.value_bytes
        {
            return None;
        }
        Some(value)
    }

    fn member(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        depth: usize,
    ) -> Option<Value> {
        self.step(depth)?;
        let object = self.expression(&member.object, depth + 1)?;
        let index = self
            .expression(&member.expression, depth + 1)?
            .as_number()?;
        if index < 0.0 || index.fract() != 0.0 {
            return None;
        }
        match object {
            Value::Array(values) => Some(values.get(index as usize)?.clone()?),
            Value::String(value) => Some(Value::String(
                value.chars().nth(index as usize)?.to_string(),
            )),
            _ => None,
        }
    }

    fn binary(&mut self, binary: &BinaryExpression<'_>, depth: usize) -> Option<Value> {
        // Reject unsupported operators before touching either operand: minified input is
        // full of comparisons that can never be folded, and their operands are pointless work.
        if !binary_operator_supported(binary.operator) {
            return None;
        }
        let string_addition = is_string_addition(binary);
        let left = self.expression(&binary.left, depth)?;
        let right = self.expression(&binary.right, depth)?;
        match binary.operator {
            BinaryOperator::Addition => {
                let (left_text, right_text) = (left.as_cow(), right.as_cow());
                if string_addition
                    && let (Some(left_text), Some(right_text)) = (&left_text, &right_text)
                {
                    let mut output = String::with_capacity(left_text.len() + right_text.len());
                    output.push_str(left_text);
                    output.push_str(right_text);
                    Some(Value::String(output))
                } else {
                    Some(Value::Number(left.as_number()? + right.as_number()?))
                }
            }
            BinaryOperator::Subtraction => {
                Some(Value::Number(left.as_number()? - right.as_number()?))
            }
            BinaryOperator::Multiplication => {
                Some(Value::Number(left.as_number()? * right.as_number()?))
            }
            BinaryOperator::Division => Some(Value::Number(left.as_number()? / right.as_number()?)),
            BinaryOperator::Remainder => {
                Some(Value::Number(left.as_number()? % right.as_number()?))
            }
            BinaryOperator::BitwiseOR => Some(Value::Number(
                ((left.as_number()? as i32) | (right.as_number()? as i32)) as f64,
            )),
            BinaryOperator::BitwiseXOR => Some(Value::Number(
                ((left.as_number()? as i32) ^ (right.as_number()? as i32)) as f64,
            )),
            BinaryOperator::BitwiseAnd => Some(Value::Number(
                ((left.as_number()? as i32) & (right.as_number()? as i32)) as f64,
            )),
            BinaryOperator::ShiftLeft => Some(Value::Number(
                ((left.as_number()? as i32) << (right.as_number()? as u32 & 31)) as f64,
            )),
            BinaryOperator::ShiftRight => Some(Value::Number(
                ((left.as_number()? as i32) >> (right.as_number()? as u32 & 31)) as f64,
            )),
            BinaryOperator::ShiftRightZeroFill => Some(Value::Number(
                ((left.as_number()? as u32) >> (right.as_number()? as u32 & 31)) as f64,
            )),
            _ => None,
        }
    }

    fn template(&mut self, template: &TemplateLiteral<'_>, depth: usize) -> Option<Value> {
        let mut output = String::new();
        for (index, quasi) in template.quasis.iter().enumerate() {
            output.push_str(quasi.value.cooked?.as_str());
            if let Some(expression) = template.expressions.get(index) {
                output.push_str(&self.expression(expression, depth)?.as_cow()?);
            }
        }
        Some(Value::String(output))
    }

    fn call(&mut self, call: &oxc_ast::ast::CallExpression<'_>, depth: usize) -> Option<Value> {
        // Resolve the callee allowlist before evaluating arguments. A minified bundle is
        // mostly calls into user functions that can never fold, so folding their arguments
        // first would be the dominant cost of the whole query.
        if !is_string_call_callee(call) {
            return None;
        }
        let from_code = call
            .callee
            .is_specific_member_access("String", "fromCharCode")
            || call
                .callee
                .is_specific_member_access("String", "fromCodePoint");
        let args: Vec<_> = call
            .arguments
            .iter()
            .map(|argument| self.expression(argument.as_expression()?, depth + 1))
            .collect::<Option<_>>()?;
        if from_code {
            let mut output = String::new();
            for argument in args {
                let code = argument.as_number()? as u32;
                output.push(char::from_u32(code)?);
            }
            return Some(Value::String(output));
        }
        let Expression::StaticMemberExpression(member) = call.callee.without_parentheses() else {
            return None;
        };
        let receiver = self.expression(&member.object, depth + 1)?;
        let object = receiver.as_cow()?;
        let name = member.property.name.as_str();
        match name {
            "toUpperCase" if args.is_empty() => Some(Value::String(object.to_uppercase())),
            "toLowerCase" if args.is_empty() => Some(Value::String(object.to_lowercase())),
            "charAt" if args.len() == 1 => Some(Value::String(
                object
                    .chars()
                    .nth(args[0].as_number()? as usize)?
                    .to_string(),
            )),
            "slice" | "substring" | "substr" if (1..=2).contains(&args.len()) => {
                let start = args[0].as_number()? as usize;
                let count = object.chars().count();
                let end = args
                    .get(1)
                    .and_then(Value::as_number)
                    .map(|value| value as usize)
                    .unwrap_or(count)
                    .min(count);
                let start = start.min(end);
                Some(Value::String(
                    object.chars().skip(start).take(end - start).collect(),
                ))
            }
            _ => None,
        }
    }
}

/// Whether `Evaluator::expression` can fold this expression's *shape* at all.
///
/// Mirrors that method's match arms exactly: keep both in sync. Deliberately one level
/// deep and allocation-free. A recursive variant was measured and rejected: minified `+`
/// chains make every nested parent re-walk its whole left spine, which costs more than the
/// evaluation it avoids.
fn is_foldable(expression: &Expression<'_>) -> bool {
    matches!(
        expression.without_parentheses(),
        Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::TemplateLiteral(_)
            | Expression::ArrayExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::UnaryExpression(_)
            | Expression::BinaryExpression(_)
            | Expression::CallExpression(_)
    )
}

fn is_string_shaped(expression: &Expression<'_>) -> bool {
    matches!(
        expression.without_parentheses(),
        Expression::StringLiteral(_) | Expression::TemplateLiteral(_)
    )
}

fn binary_operator_supported(operator: BinaryOperator) -> bool {
    matches!(
        operator,
        BinaryOperator::Addition
            | BinaryOperator::Subtraction
            | BinaryOperator::Multiplication
            | BinaryOperator::Division
            | BinaryOperator::Remainder
            | BinaryOperator::BitwiseOR
            | BinaryOperator::BitwiseXOR
            | BinaryOperator::BitwiseAnd
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight
            | BinaryOperator::ShiftRightZeroFill
    )
}

/// Shallow structural test used by the evaluator itself: does `+` concatenate here?
fn is_string_addition(binary: &BinaryExpression<'_>) -> bool {
    binary.operator == BinaryOperator::Addition
        && (is_string_shaped(&binary.left) || is_string_shaped(&binary.right))
}

/// Shallow structural test used by the evaluator itself: is this callee one it can fold?
fn is_string_call_callee(call: &oxc_ast::ast::CallExpression<'_>) -> bool {
    if call.optional || call.arguments.iter().any(Argument::is_spread) {
        return false;
    }
    if call
        .callee
        .is_specific_member_access("String", "fromCharCode")
        || call
            .callee
            .is_specific_member_access("String", "fromCodePoint")
    {
        return true;
    }
    match call.callee.without_parentheses() {
        Expression::StaticMemberExpression(member) => matches!(
            member.property.name.as_str(),
            "toUpperCase" | "toLowerCase" | "charAt" | "slice" | "substring" | "substr"
        ),
        _ => false,
    }
}

/// Visitor-side guard: can this `+` node possibly fold to a string?
pub fn binary_can_yield_string(binary: &BinaryExpression<'_>) -> bool {
    is_string_addition(binary) && is_foldable(&binary.left) && is_foldable(&binary.right)
}

/// Visitor-side guard: can this call possibly fold to a string value?
///
/// Sound as a skip condition: [`Evaluator::call`] produces a string only through
/// `String.fromCharCode`/`fromCodePoint` or one of the string-method names, and in every
/// case all arguments and (for methods) the receiver must fold first.
pub fn call_can_yield_string(call: &oxc_ast::ast::CallExpression<'_>) -> bool {
    if !is_string_call_callee(call) {
        return false;
    }
    if !call
        .arguments
        .iter()
        .all(|argument| argument.as_expression().is_some_and(is_foldable))
    {
        return false;
    }
    // `String.fromCharCode`/`fromCodePoint` read only their arguments, so their receiver
    // is never evaluated; a string method must additionally fold its receiver.
    if call
        .callee
        .is_specific_member_access("String", "fromCharCode")
        || call
            .callee
            .is_specific_member_access("String", "fromCodePoint")
    {
        return true;
    }
    match call.callee.without_parentheses() {
        Expression::StaticMemberExpression(member) => is_foldable(&member.object),
        _ => true,
    }
}

/// Visitor-side guard: can this computed access possibly fold to a string value?
pub fn member_can_yield_string(member: &oxc_ast::ast::ComputedMemberExpression<'_>) -> bool {
    is_foldable(&member.object) && is_foldable(&member.expression)
}

/// Visitor-side guard: can this template literal possibly fold to a string value?
pub fn template_can_yield_string(template: &TemplateLiteral<'_>) -> bool {
    template
        .quasis
        .iter()
        .all(|quasi| quasi.value.cooked.is_some())
        && template.expressions.iter().all(is_foldable)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Null => false,
        Value::Number(value) => *value != 0.0 && !value.is_nan(),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) => true,
    }
}
