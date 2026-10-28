use crate::Name;
use oxc_allocator::{Allocator, ArenaVec};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_span::SPAN;
use oxc_str::{Ident, Str};
use oxc_syntax::{
    number::NumberBase,
    operator::{AssignmentOperator, BinaryOperator, UnaryOperator},
};

pub(super) struct Ast<'a> {
    pub build: AstBuilder<'a>,
}
impl<'a> Ast<'a> {
    pub fn new(arena: &'a Allocator) -> Self {
        Self {
            build: AstBuilder::new(arena),
        }
    }
    pub fn vec<T>(&self, values: impl IntoIterator<Item = T>) -> ArenaVec<'a, T> {
        let mut result = ArenaVec::new_in(&self.build);
        result.extend(values);
        result
    }
    pub fn id(&self, name: &str) -> Expression<'a> {
        Expression::new_identifier(SPAN, Ident::from_str_in(name, &self.build), &self.build)
    }
    pub fn binding(&self, name: &str) -> BindingIdentifier<'a> {
        BindingIdentifier::new(SPAN, Ident::from_str_in(name, &self.build), &self.build)
    }
    pub fn pattern(&self, name: &str) -> BindingPattern<'a> {
        BindingPattern::BindingIdentifier(oxc_allocator::ArenaBox::new_in(
            self.binding(name),
            &self.build,
        ))
    }
    pub fn string(&self, text: &str) -> Expression<'a> {
        Expression::new_string_literal(SPAN, Str::from_str_in(text, &self.build), None, &self.build)
    }
    pub fn name(&self, name: &Name) -> Expression<'a> {
        if let Some(text) = name.scalar_text() {
            return self.string(text);
        }
        use std::fmt::Write;
        let mut text = String::new();
        for unit in std::char::decode_utf16(name.units.iter().copied()) {
            match unit {
                Ok('\u{fffd}') => text.push_str("\u{fffd}fffd"),
                Ok(ch) => text.push(ch),
                Err(error) => {
                    write!(text, "\u{fffd}{:04x}", error.unpaired_surrogate())
                        .expect("string write");
                }
            }
        }
        Expression::StringLiteral(StringLiteral::boxed_with_lone_surrogates(
            SPAN,
            Str::from_str_in(&text, &self.build),
            None,
            true,
            &self.build,
        ))
    }
    pub fn number(&self, value: f64) -> Expression<'a> {
        if value.is_nan() {
            return self.binary(self.number(0.0), BinaryOperator::Division, self.number(0.0));
        }
        if value.is_infinite() {
            let infinity =
                self.binary(self.number(1.0), BinaryOperator::Division, self.number(0.0));
            return if value.is_sign_negative() {
                self.unary(UnaryOperator::UnaryNegation, infinity)
            } else {
                infinity
            };
        }
        if value.is_sign_negative() {
            return self.unary(UnaryOperator::UnaryNegation, self.number(-value));
        }
        Expression::new_numeric_literal(SPAN, value, None, NumberBase::Decimal, &self.build)
    }
    pub fn boolean(&self, value: bool) -> Expression<'a> {
        Expression::new_boolean_literal(SPAN, value, &self.build)
    }
    pub fn undefined(&self) -> Expression<'a> {
        self.unary(UnaryOperator::Void, self.number(0.0))
    }
    pub fn unary(&self, op: UnaryOperator, value: Expression<'a>) -> Expression<'a> {
        Expression::new_unary_expression(SPAN, op, value, &self.build)
    }
    pub fn binary(
        &self,
        left: Expression<'a>,
        op: BinaryOperator,
        right: Expression<'a>,
    ) -> Expression<'a> {
        Expression::new_binary_expression(SPAN, left, op, right, &self.build)
    }
    pub fn member(&self, object: Expression<'a>, key: Expression<'a>) -> Expression<'a> {
        Expression::new_computed_member_expression(SPAN, object, key, false, &self.build)
    }
    pub fn property(&self, object: Expression<'a>, key: &str) -> Expression<'a> {
        self.member(object, self.string(key))
    }
    pub fn call(&self, callee: Expression<'a>, args: Vec<Expression<'a>>) -> Expression<'a> {
        Expression::new_call_expression(
            SPAN,
            callee,
            None,
            self.vec(args.into_iter().map(Argument::from)),
            false,
            &self.build,
        )
    }
    pub fn construct(&self, callee: Expression<'a>, args: Vec<Expression<'a>>) -> Expression<'a> {
        Expression::new_new_expression(
            SPAN,
            callee,
            None,
            self.vec(args.into_iter().map(Argument::from)),
            &self.build,
        )
    }
    pub fn array(&self, values: Vec<Expression<'a>>) -> Expression<'a> {
        Expression::new_array_expression(
            SPAN,
            self.vec(values.into_iter().map(ArrayExpressionElement::from)),
            &self.build,
        )
    }
    pub fn object(&self, entries: Vec<(Expression<'a>, Expression<'a>)>) -> Expression<'a> {
        let entries = entries.into_iter().map(|(key, value)| {
            ObjectPropertyKind::new_object_property(
                SPAN,
                PropertyKind::Init,
                PropertyKey::from(key),
                value,
                false,
                false,
                true,
                &self.build,
            )
        });
        Expression::new_object_expression(SPAN, self.vec(entries), &self.build)
    }
    pub fn target(&self, expression: Expression<'a>) -> AssignmentTarget<'a> {
        match expression {
            Expression::Identifier(id) => AssignmentTarget::AssignmentTargetIdentifier(id),
            Expression::ComputedMemberExpression(member) => {
                AssignmentTarget::ComputedMemberExpression(member)
            }
            Expression::StaticMemberExpression(member) => {
                AssignmentTarget::StaticMemberExpression(member)
            }
            _ => unreachable!("typed Ark assignment target"),
        }
    }
    pub fn assign(&self, target: Expression<'a>, value: Expression<'a>) -> Statement<'a> {
        self.expr(Expression::new_assignment_expression(
            SPAN,
            AssignmentOperator::Assign,
            self.target(target),
            value,
            &self.build,
        ))
    }
    pub fn expr(&self, expr: Expression<'a>) -> Statement<'a> {
        Statement::new_expression_statement(SPAN, expr, &self.build)
    }
    pub fn block(&self, body: Vec<Statement<'a>>) -> Statement<'a> {
        Statement::new_block_statement(SPAN, self.vec(body), &self.build)
    }
    pub fn if_(
        &self,
        test: Expression<'a>,
        yes: Vec<Statement<'a>>,
        no: Option<Vec<Statement<'a>>>,
    ) -> Statement<'a> {
        Statement::new_if_statement(
            SPAN,
            test,
            self.block(yes),
            no.map(|body| self.block(body)),
            &self.build,
        )
    }
    pub fn ret(&self, value: Option<Expression<'a>>) -> Statement<'a> {
        Statement::new_return_statement(SPAN, value, &self.build)
    }
    pub fn throw(&self, value: Expression<'a>) -> Statement<'a> {
        Statement::new_throw_statement(SPAN, value, &self.build)
    }
    pub fn jump(&self, label: &str, is_continue: bool) -> Statement<'a> {
        let label = Some(LabelIdentifier::new(
            SPAN,
            Ident::from_str_in(label, &self.build),
            &self.build,
        ));
        if is_continue {
            Statement::new_continue_statement(SPAN, label, &self.build)
        } else {
            Statement::new_break_statement(SPAN, label, &self.build)
        }
    }
    pub fn label(&self, label: &str, body: Statement<'a>) -> Statement<'a> {
        Statement::new_labeled_statement(
            SPAN,
            LabelIdentifier::new(SPAN, Ident::from_str_in(label, &self.build), &self.build),
            body,
            &self.build,
        )
    }
    pub fn var(&self, name: &str, value: Option<Expression<'a>>, constant: bool) -> Statement<'a> {
        let pattern = BindingPattern::BindingIdentifier(oxc_allocator::ArenaBox::new_in(
            self.binding(name),
            &self.build,
        ));
        let decl = VariableDeclarator::new(SPAN, pattern, None, value, false, &self.build);
        Statement::new_variable_declaration(
            SPAN,
            if constant {
                VariableDeclarationKind::Const
            } else {
                VariableDeclarationKind::Let
            },
            self.vec([decl]),
            false,
            &self.build,
        )
    }
    pub fn parameters(
        &self,
        names: Vec<String>,
    ) -> oxc_allocator::ArenaBox<'a, FormalParameters<'a>> {
        let params = names.into_iter().map(|name| {
            FormalParameter::new(
                SPAN,
                self.vec([]),
                BindingPattern::BindingIdentifier(oxc_allocator::ArenaBox::new_in(
                    self.binding(&name),
                    &self.build,
                )),
                None,
                None,
                false,
                None,
                false,
                false,
                &self.build,
            )
        });
        FormalParameters::boxed(
            SPAN,
            FormalParameterKind::FormalParameter,
            self.vec(params),
            None,
            &self.build,
        )
    }
    pub fn function(
        &self,
        name: &str,
        params: Vec<String>,
        body: Vec<Statement<'a>>,
        asynchronous: bool,
    ) -> Expression<'a> {
        let body = FunctionBody::boxed(
            SPAN,
            self.vec([Directive::new_use_strict(&self.build)]),
            self.vec(body),
            &self.build,
        );
        Expression::new_function_expression(
            SPAN,
            FunctionType::FunctionExpression,
            Some(self.binding(name)),
            false,
            asynchronous,
            false,
            None,
            None,
            self.parameters(params),
            None,
            Some(body),
            &self.build,
        )
    }
    pub fn factory(
        &self,
        name: &str,
        params: Vec<String>,
        body: Vec<Statement<'a>>,
    ) -> Statement<'a> {
        let body = FunctionBody::boxed(
            SPAN,
            self.vec([Directive::new_use_strict(&self.build)]),
            self.vec(body),
            &self.build,
        );
        Statement::new_function_declaration(
            SPAN,
            FunctionType::FunctionDeclaration,
            Some(self.binding(name)),
            false,
            false,
            false,
            None,
            None,
            self.parameters(params),
            None,
            Some(body),
            &self.build,
        )
    }
}
