//! Bounded syntax-local evaluation under standard-intrinsic semantics.
//! Never executes input, follows bindings, or performs I/O. Strings stay JS UTF-16.

use crate::strings::JsString;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BinaryExpression, Expression, TemplateLiteral,
};
use oxc_syntax::{
    number::ToJsString,
    operator::{BinaryOperator, UnaryOperator},
};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(JsString),
    Number(f64),
    Bool(bool),
    Null,
    Array(Vec<Option<Value>>),
}

impl Value {
    fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Bool(b) => Some(u8::from(*b) as f64),
            Self::Null => Some(0.0),
            Self::String(s) => {
                let text = s.to_utf8()?;
                // Only the unambiguous ASCII decimal subset. Do not substitute Rust's
                // whitespace, radix or case-insensitive infinity rules for ECMAScript.
                if !text.is_ascii() {
                    return None;
                }
                let text = text.trim_matches([' ', '\t', '\r', '\n', '\u{b}', '\u{c}']);
                match text {
                    "" => Some(0.0),
                    "Infinity" | "+Infinity" => Some(f64::INFINITY),
                    "-Infinity" => Some(f64::NEG_INFINITY),
                    _ if text
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) =>
                    {
                        text.parse().ok()
                    }
                    _ => None,
                }
            }
            Self::Array(_) => None,
        }
    }

    pub(crate) fn string(&self) -> Option<JsString> {
        Some(match self {
            Self::String(s) => s.clone(),
            Self::Number(n) => JsString::from(n.to_js_string().as_str()),
            Self::Bool(b) => JsString::from(if *b { "true" } else { "false" }),
            Self::Null => JsString::from("null"),
            Self::Array(_) => return None,
        })
    }

    fn bytes(&self) -> usize {
        match self {
            Self::String(s) => s.len_utf16() * 2,
            Self::Array(a) => a
                .iter()
                .map(|v| v.as_ref().map_or(0, Self::bytes) + 8)
                .sum(),
            _ => 8,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Limits {
    pub depth: usize,
    pub steps: usize,
    pub value_bytes: usize,
    pub allow_intrinsics: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 32,
            steps: 512,
            value_bytes: 64 * 1024,
            allow_intrinsics: true,
        }
    }
}

pub fn evaluate(expression: &Expression<'_>, limits: Limits) -> Option<Value> {
    Context {
        limits,
        decoders: None,
    }
    .evaluate(expression)
    .map(|v| v.value)
}

pub(crate) struct Evaluated {
    pub value: Value,
    pub decoded: bool,
}
impl Evaluated {
    pub fn resolution(&self) -> &'static str {
        if self.decoded { "decoder" } else { "static" }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Context<'a> {
    pub limits: Limits,
    pub decoders: Option<&'a crate::Decoders>,
}
impl<'a> Context<'a> {
    fn run(self, f: impl FnOnce(&mut Evaluator<'a>) -> Option<Value>) -> Option<Evaluated> {
        let mut evaluator = Evaluator {
            limits: self.limits,
            steps: 0,
            decoders: self.decoders,
            decoded: false,
        };
        let value = f(&mut evaluator)?;
        Some(Evaluated {
            value,
            decoded: evaluator.decoded,
        })
    }
    pub fn evaluate(self, expression: &Expression<'_>) -> Option<Evaluated> {
        self.run(|e| e.expression(expression, 0))
    }
    pub fn binary(self, expression: &BinaryExpression<'_>) -> Option<Evaluated> {
        self.run(|e| e.binary(expression, 0))
    }
    pub fn call(self, expression: &oxc_ast::ast::CallExpression<'_>) -> Option<Evaluated> {
        self.run(|e| e.call(expression, 0))
    }
    pub fn template(self, expression: &TemplateLiteral<'_>) -> Option<Evaluated> {
        self.run(|e| e.template(expression, 0))
    }
    pub fn member(
        self,
        expression: &oxc_ast::ast::ComputedMemberExpression<'_>,
    ) -> Option<Evaluated> {
        self.run(|e| e.member(expression, 0))
    }
}

struct Evaluator<'a> {
    limits: Limits,
    steps: usize,
    decoders: Option<&'a crate::Decoders>,
    decoded: bool,
}
impl Evaluator<'_> {
    fn step(&mut self, depth: usize) -> Option<()> {
        if depth > self.limits.depth || self.steps >= self.limits.steps {
            return None;
        }
        self.steps += 1;
        Some(())
    }
    fn bounded(&self, value: Value) -> Option<Value> {
        (value.bytes() <= self.limits.value_bytes).then_some(value)
    }
    fn decoded(&self, text: &str, lone: bool) -> Option<JsString> {
        if text.len() > self.limits.value_bytes.saturating_mul(4) {
            return None;
        }
        let s = JsString::decoded(text, lone);
        (s.len_utf16() <= self.limits.value_bytes / 2).then_some(s)
    }
    fn expression(&mut self, expression: &Expression<'_>, depth: usize) -> Option<Value> {
        self.step(depth)?;
        let value = match expression.get_inner_expression() {
            Expression::StringLiteral(l) => {
                Value::String(self.decoded(l.value.as_str(), l.lone_surrogates)?)
            }
            Expression::NumericLiteral(l) => Value::Number(l.value),
            Expression::BooleanLiteral(l) => Value::Bool(l.value),
            Expression::NullLiteral(_) => Value::Null,
            Expression::TemplateLiteral(t) => self.template(t, depth + 1)?,
            Expression::ArrayExpression(a) => {
                if a.elements.len() > self.limits.steps.saturating_sub(self.steps) {
                    return None;
                }
                let mut values = Vec::new();
                let mut bytes = 0;
                for element in &a.elements {
                    self.step(depth + 1)?;
                    let value = match element {
                        ArrayExpressionElement::Elision(_) => None,
                        ArrayExpressionElement::SpreadElement(_) => return None,
                        e => Some(self.expression(e.as_expression()?, depth + 1)?),
                    };
                    bytes += value.as_ref().map_or(0, Value::bytes) + 8;
                    if bytes > self.limits.value_bytes {
                        return None;
                    }
                    values.push(value);
                }
                Value::Array(values)
            }
            Expression::ComputedMemberExpression(m) => self.member(m, depth + 1)?,
            Expression::UnaryExpression(u) => {
                let v = self.expression(&u.argument, depth + 1)?;
                match u.operator {
                    UnaryOperator::UnaryPlus => Value::Number(v.as_number()?),
                    UnaryOperator::UnaryNegation => Value::Number(-v.as_number()?),
                    UnaryOperator::LogicalNot => Value::Bool(!truthy(&v)),
                    UnaryOperator::BitwiseNot => {
                        Value::Number((!(uint32(v.as_number()?) as i32)) as f64)
                    }
                    _ => return None,
                }
            }
            Expression::BinaryExpression(b) => self.binary(b, depth + 1)?,
            Expression::CallExpression(c) => self.call(c, depth + 1)?,
            _ => return None,
        };
        self.bounded(value)
    }

    fn member(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        depth: usize,
    ) -> Option<Value> {
        self.step(depth)?;
        let object = self.expression(&member.object, depth + 1)?;
        let key = self.expression(&member.expression, depth + 1)?;
        let index = match &key {
            Value::Number(n) => *n,
            Value::String(s) => {
                let n = key.as_number()?;
                if !s.equals_str(&n.to_js_string()) {
                    return None;
                }
                n
            }
            _ => return None,
        };
        if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
            return None;
        }
        match object {
            Value::Array(a) => a.get(index as usize)?.clone(),
            Value::String(s) => Some(Value::String(JsString::from_units(vec![
                *s.units().get(index as usize)?,
            ]))),
            _ => None,
        }
    }

    fn binary(&mut self, binary: &BinaryExpression<'_>, depth: usize) -> Option<Value> {
        self.step(depth)?;
        if !binary_operator_supported(binary.operator) {
            return None;
        }
        let l = self.expression(&binary.left, depth + 1)?;
        let r = self.expression(&binary.right, depth + 1)?;
        if binary.operator == BinaryOperator::Addition
            && (matches!(l, Value::String(_)) || matches!(r, Value::String(_)))
        {
            let mut value = l.string()?;
            value.append_bounded(&r.string()?, self.limits.value_bytes / 2)?;
            return Some(Value::String(value));
        }
        let (l, r) = (l.as_number()?, r.as_number()?);
        let n = match binary.operator {
            BinaryOperator::Addition => l + r,
            BinaryOperator::Subtraction => l - r,
            BinaryOperator::Multiplication => l * r,
            BinaryOperator::Division => l / r,
            BinaryOperator::Remainder => l % r,
            BinaryOperator::BitwiseOR => ((uint32(l) | uint32(r)) as i32) as f64,
            BinaryOperator::BitwiseXOR => ((uint32(l) ^ uint32(r)) as i32) as f64,
            BinaryOperator::BitwiseAnd => ((uint32(l) & uint32(r)) as i32) as f64,
            BinaryOperator::ShiftLeft => ((uint32(l) << (uint32(r) & 31)) as i32) as f64,
            BinaryOperator::ShiftRight => ((uint32(l) as i32) >> (uint32(r) & 31)) as f64,
            BinaryOperator::ShiftRightZeroFill => (uint32(l) >> (uint32(r) & 31)) as f64,
            _ => return None,
        };
        self.bounded(Value::Number(n))
    }

    fn template(&mut self, template: &TemplateLiteral<'_>, depth: usize) -> Option<Value> {
        self.step(depth)?;
        let mut result = JsString::from("");
        for (i, q) in template.quasis.iter().enumerate() {
            self.step(depth + 1)?;
            result.append_bounded(
                &self.decoded(q.value.cooked?.as_str(), q.lone_surrogates)?,
                self.limits.value_bytes / 2,
            )?;
            if let Some(e) = template.expressions.get(i) {
                result.append_bounded(
                    &self.expression(e, depth + 1)?.string()?,
                    self.limits.value_bytes / 2,
                )?;
            }
        }
        Some(Value::String(result))
    }

    fn call(&mut self, call: &oxc_ast::ast::CallExpression<'_>, depth: usize) -> Option<Value> {
        self.step(depth)?;
        if let Some(value) = self.decoders.and_then(|d| d.call(call)) {
            if value.value.len_utf16() > self.limits.value_bytes / 2 {
                return None;
            }
            self.decoded = true;
            return Some(Value::String(value.value.clone()));
        }
        if !self.limits.allow_intrinsics
            || !is_string_call_callee(call)
            || call.arguments.len() > self.limits.steps.saturating_sub(self.steps)
        {
            return None;
        }
        let args = call
            .arguments
            .iter()
            .map(|a| self.expression(a.as_expression()?, depth + 1))
            .collect::<Option<Vec<_>>>()?;
        let from_unit = call
            .callee
            .is_specific_member_access("String", "fromCharCode");
        let from_point = call
            .callee
            .is_specific_member_access("String", "fromCodePoint");
        if from_unit || from_point {
            let mut units = Vec::new();
            for arg in args {
                let n = arg.as_number()?;
                if from_unit {
                    units.push(uint32(n) as u16);
                } else {
                    if !n.is_finite() || !(0.0..=0x10ffff as f64).contains(&n) || n.fract() != 0.0 {
                        return None;
                    }
                    let n = n as u32;
                    if n <= 0xffff {
                        units.push(n as u16);
                    } else {
                        units.push(0xd800 + ((n - 0x10000) >> 10) as u16);
                        units.push(0xdc00 + ((n - 0x10000) & 1023) as u16);
                    }
                }
                if units.len() > self.limits.value_bytes / 2 {
                    return None;
                }
            }
            return Some(Value::String(JsString::from_units(units)));
        }
        let Expression::StaticMemberExpression(m) = call.callee.get_inner_expression() else {
            return None;
        };
        // A Number/Boolean/Array does not acquire String's prototype methods through
        // ToString. Only a known primitive string is a supported receiver.
        let Value::String(object) = self.expression(&m.object, depth + 1)? else {
            return None;
        };
        let units = object.units();
        let len = units.len();
        let result = match m.property.name.as_str() {
            "toUpperCase" | "toLowerCase" if args.is_empty() => {
                // Unicode case tables and contextual casing are not approximated.
                if units.iter().any(|u| *u > 127) {
                    return None;
                }
                JsString::from_units(
                    units
                        .iter()
                        .map(|u| {
                            let c = *u as u8;
                            if m.property.name == "toUpperCase" {
                                c.to_ascii_uppercase() as u16
                            } else {
                                c.to_ascii_lowercase() as u16
                            }
                        })
                        .collect(),
                )
            }
            "charAt" if args.len() <= 1 => {
                let i = integer(
                    args.first()
                        .map(Value::as_number)
                        .transpose_option()?
                        .unwrap_or(0.0),
                );
                let unit = if i >= 0.0 && i < len as f64 {
                    units.get(i as usize).copied()
                } else {
                    None
                };
                JsString::from_units(unit.into_iter().collect())
            }
            "slice" | "substring" | "substr" if args.len() <= 2 => {
                let a = args
                    .first()
                    .map(Value::as_number)
                    .transpose_option()?
                    .unwrap_or(0.0);
                let b = args.get(1).map(Value::as_number).transpose_option()?;
                let (start, end) = match m.property.name.as_str() {
                    "slice" => (relative(a, len), b.map_or(len, |b| relative(b, len))),
                    "substring" => {
                        let a = clamp(a, len);
                        let b = b.map_or(len, |b| clamp(b, len));
                        (a.min(b), a.max(b))
                    }
                    _ => {
                        let a = relative(a, len);
                        (a, a + b.map_or(len - a, |b| clamp(b, len - a)))
                    }
                };
                JsString::from_units(units[start..end.max(start)].to_vec())
            }
            _ => return None,
        };
        self.bounded(Value::String(result))
    }
}

// Keep absent arguments distinct from supplied expressions we cannot evaluate.
trait TransposeOption<T> {
    fn transpose_option(self) -> Option<Option<T>>;
}
impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Option<Option<T>> {
        self.map_or(Some(None), |v| v.map(Some))
    }
}
fn integer(n: f64) -> f64 {
    if n.is_nan() { 0.0 } else { n.trunc() }
}
fn clamp(n: f64, len: usize) -> usize {
    integer(n).max(0.0).min(len as f64) as usize
}
fn relative(n: f64, len: usize) -> usize {
    let n = integer(n);
    if n < 0.0 {
        (len as f64 + n).max(0.0) as usize
    } else {
        clamp(n, len)
    }
}
fn uint32(n: f64) -> u32 {
    if !n.is_finite() || n == 0.0 {
        0
    } else {
        n.trunc().rem_euclid(4294967296.0) as u32
    }
}
fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => *n != 0.0 && !n.is_nan(),
        Value::String(s) => s.len_utf16() != 0,
        Value::Array(_) => true,
    }
}

fn is_foldable(e: &Expression<'_>) -> bool {
    matches!(
        e.get_inner_expression(),
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
fn binary_operator_supported(op: BinaryOperator) -> bool {
    matches!(
        op,
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
fn is_string_call_callee(c: &oxc_ast::ast::CallExpression<'_>) -> bool {
    if c.optional || c.arguments.iter().any(Argument::is_spread) {
        return false;
    }
    if c.callee.is_specific_member_access("String", "fromCharCode")
        || c.callee
            .is_specific_member_access("String", "fromCodePoint")
    {
        return true;
    }
    matches!(c.callee.get_inner_expression(), Expression::StaticMemberExpression(m) if matches!(m.property.name.as_str(), "toUpperCase" | "toLowerCase" | "charAt" | "slice" | "substring" | "substr"))
}
/// Disable intrinsic assumptions under visible shadowing/dynamic scope.
/// This does not prove external code preserves prototypes.
pub(crate) fn intrinsics_allowed(
    program: &oxc_ast::ast::Program<'_>,
    max_depth: usize,
    decoders: Option<&crate::Decoders>,
) -> bool {
    use oxc_ast::AstKind;
    use oxc_ast_visit::Visit;
    use oxc_span::GetSpan;
    const CAPABILITY_KEYS: &[&str] = &[
        "prototype",
        "__proto__",
        "getPrototypeOf",
        "setPrototypeOf",
        "defineProperty",
        "defineProperties",
        "getOwnPropertyDescriptor",
        "getOwnPropertyDescriptors",
        "__defineGetter__",
        "__defineSetter__",
        "__lookupGetter__",
        "__lookupSetter__",
        "constructor",
        "require",
    ];
    struct Guard<'a, 'd> {
        parents: Vec<AstKind<'a>>,
        allowed: bool,
        max_depth: usize,
        evaluation: Context<'d>,
    }
    impl<'a> Visit<'a> for Guard<'a, '_> {
        fn enter_node(&mut self, node: AstKind<'a>) {
            if self.parents.len() >= self.max_depth {
                std::panic::panic_any(rxd_core::Error::new(
                    "RESOURCE_LIMIT",
                    "AST depth budget exceeded",
                ));
            }
            if !self.allowed {
                self.parents.push(node);
                return;
            }
            let capability_key = |expression: &Expression<'_>| {
                self.evaluation
                    .evaluate(expression)
                    .and_then(|v| v.value.string())
                    .map(|key| CAPABILITY_KEYS.iter().any(|name| key.equals_str(name)))
            };
            self.allowed &= !match node {
                AstKind::StaticMemberExpression(m) => {
                    CAPABILITY_KEYS.contains(&m.property.name.as_str())
                }
                AstKind::ComputedMemberExpression(m) => {
                    capability_key(&m.expression).unwrap_or(false)
                }
                AstKind::BindingProperty(p) if p.computed => p
                    .key
                    .as_expression()
                    .and_then(capability_key)
                    .unwrap_or(true),
                AstKind::BindingProperty(p) => p
                    .key
                    .static_name()
                    .is_some_and(|key| CAPABILITY_KEYS.contains(&key.as_ref())),
                _ => false,
            };
            match node {
                AstKind::BindingIdentifier(id) if id.name == "String" => self.allowed = false,
                AstKind::WithStatement(_)
                | AstKind::ThisExpression(_)
                | AstKind::ImportExpression(_)
                | AstKind::TSImportEqualsDeclaration(_) => self.allowed = false,
                AstKind::ImportDeclaration(d)
                    if d.import_kind != oxc_ast::ast::ImportOrExportKind::Type =>
                {
                    self.allowed = false
                }
                AstKind::ExportFromDeclaration(d)
                    if d.export_kind != oxc_ast::ast::ImportOrExportKind::Type =>
                {
                    self.allowed = false
                }
                AstKind::ExportAllDeclaration(d)
                    if d.export_kind != oxc_ast::ast::ImportOrExportKind::Type =>
                {
                    self.allowed = false
                }
                AstKind::IdentifierReference(id)
                    if crate::decode::GLOBAL_NAMES.contains(&id.name.as_str())
                        || matches!(
                            id.name.as_str(),
                            "eval" | "Function" | "Reflect" | "require"
                        ) =>
                {
                    self.allowed = false
                }
                AstKind::IdentifierReference(id) if id.name == "String" => {
                    let direct = match (self.parents.last(), self.parents.iter().rev().nth(1)) {
                        (
                            Some(AstKind::StaticMemberExpression(m)),
                            Some(AstKind::CallExpression(c)),
                        ) => {
                            m.object.span() == id.span
                                && c.callee.span() == m.span
                                && matches!(
                                    m.property.name.as_str(),
                                    "fromCharCode" | "fromCodePoint"
                                )
                        }
                        _ => false,
                    };
                    self.allowed &= direct;
                }
                _ => {}
            }
            self.parents.push(node);
        }
        fn leave_node(&mut self, _: AstKind<'a>) {
            self.parents.pop();
        }
    }
    let mut guard = Guard {
        parents: Vec::new(),
        allowed: true,
        max_depth,
        evaluation: Context {
            limits: Limits {
                allow_intrinsics: false,
                ..Default::default()
            },
            decoders,
        },
    };
    guard.visit_program(program);
    guard.allowed
}

// One-level guards only: recursive prechecks re-walk minified concatenation spines.
pub fn binary_can_yield_string(b: &BinaryExpression<'_>) -> bool {
    b.operator == BinaryOperator::Addition && is_foldable(&b.left) && is_foldable(&b.right)
}
pub fn call_can_yield_string(c: &oxc_ast::ast::CallExpression<'_>) -> bool {
    if !is_string_call_callee(c)
        || !c
            .arguments
            .iter()
            .all(|a| a.as_expression().is_some_and(is_foldable))
    {
        return false;
    }
    if c.callee.is_specific_member_access("String", "fromCharCode")
        || c.callee
            .is_specific_member_access("String", "fromCodePoint")
    {
        return true;
    }
    matches!(c.callee.get_inner_expression(), Expression::StaticMemberExpression(m) if is_foldable(&m.object))
}
pub fn member_can_yield_string(m: &oxc_ast::ast::ComputedMemberExpression<'_>) -> bool {
    is_foldable(&m.object) && is_foldable(&m.expression)
}
pub fn template_can_yield_string(t: &TemplateLiteral<'_>) -> bool {
    t.quasis.iter().all(|q| q.value.cooked.is_some()) && t.expressions.iter().all(is_foldable)
}
