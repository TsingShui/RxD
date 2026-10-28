use crate::{reader::resource, structure::unsupported};
use oxc_allocator::{Allocator, ArenaVec, CloneIn, TakeIn};
use oxc_ast::{ast::*, builder::AstBuilder};
use oxc_ast_visit::{Visit, VisitMut, walk, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::{GetSpan, SourceType};
use oxc_syntax::{
    operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator},
    scope::{ScopeFlags, ScopeId},
    symbol::SymbolId,
};
use rxd_core::{Budget, Result, output::Output};
use std::{collections::HashMap, io::Write};

pub(crate) fn compact(source: Vec<u8>, budget: Budget) -> Result<Vec<u8>> {
    admit(&source, budget)?;
    let source = String::from_utf8(source).map_err(|_| unsupported("invalid generated UTF-8"))?;
    let allocator = Allocator::new();
    let parsed = Parser::new(&allocator, &source, SourceType::mjs())
        .with_options(ParseOptions {
            preserve_parens: false,
            ..ParseOptions::default()
        })
        .parse();
    if !parsed.diagnostics.is_empty() || parsed.fatal_error {
        return Err(unsupported(format!(
            "generated source syntax: {:?}",
            parsed.diagnostics
        )));
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new_compiler().build(&program);
    if !semantic.diagnostics.is_empty() {
        return Err(unsupported(format!(
            "generated source bindings: {:?}",
            semantic.diagnostics
        )));
    }
    let scoping = semantic.semantic.into_scoping();
    let enabled = !scoping.scope_descendants_from_root().any(|scope| {
        let flags = scoping.scope_flags(scope);
        flags.contains_direct_eval() || flags.is_with()
    });
    if enabled {
        let mut context = Context::new(&allocator, scoping)?;
        for statement in &program.body {
            if let Statement::ExportNamedDeclaration(export) = statement {
                for specifier in &export.specifiers {
                    if let ModuleExportName::IdentifierReference(reference) = &specifier.local
                        && let Some(id) = context.symbol(reference)
                    {
                        context.locals[id.index()] = false;
                    }
                }
            }
        }
        for _ in 0..4 {
            let mut forward = Forward {
                context: &context,
                env: HashMap::new(),
                changed: false,
            };
            forward.visit_program(&mut program);
            let mut reads = Uses::new(&context, true)?;
            reads.visit_program(&program);
            let mut cleanup = Cleanup {
                context: &context,
                reads: &reads.counts,
                this_pure: true,
                derived_class: false,
                changed: false,
            };
            cleanup.visit_program(&mut program);
            let mut remaining = Uses::new(&context, false)?;
            remaining.visit_program(&program);
            let mut declarations = Declarations {
                context: &context,
                uses: &remaining.counts,
                this_pure: true,
                derived_class: false,
                changed: false,
            };
            declarations.visit_program(&mut program);
            if allocator.used_bytes() > budget.metadata_bytes {
                return Err(resource("ABC source AST exceeds metadata admission"));
            }
            if !forward.changed && !cleanup.changed && !declarations.changed {
                break;
            }
        }
    }
    if allocator.used_bytes() > budget.metadata_bytes {
        return Err(resource("ABC source AST exceeds metadata admission"));
    }
    let code = Codegen::new()
        .with_options(CodegenOptions {
            comments: CommentOptions::disabled(),
            ..CodegenOptions::default()
        })
        .build(&program)
        .code;
    let mut output = Output::new(budget.output_bytes);
    output.write_all(code.as_bytes())?;
    Ok(output.into_bytes())
}

// Admission is for generated AST metadata, not an additional input or output quota.
fn admit(source: &[u8], budget: Budget) -> Result<()> {
    source
        .len()
        .checked_mul(32)
        .filter(|bytes| *bytes <= budget.metadata_bytes)
        .ok_or_else(|| resource("ABC source AST admission exceeds metadata budget"))?;
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0usize;
    for &byte in source {
        if let Some(mark) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == mark {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'(' | b'{' | b'[' => {
                depth += 1;
                if depth > budget.ast_depth {
                    return Err(resource("ABC source AST nesting exceeds depth guard"));
                }
            }
            b')' | b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

struct Context<'a> {
    ast: AstBuilder<'a>,
    allocator: &'a Allocator,
    scoping: Scoping,
    locals: Vec<bool>,
}

impl<'a> Context<'a> {
    fn new(allocator: &'a Allocator, scoping: Scoping) -> Result<Self> {
        let mut locals = Vec::new();
        locals
            .try_reserve_exact(scoping.symbols_len())
            .map_err(|_| resource("ABC AST symbol allocation failed"))?;
        locals.resize(scoping.symbols_len(), false);
        for id in scoping.symbol_ids() {
            let scope = scoping.symbol_scope_id(id);
            if !(scoping.scope_flags(scope).is_function() || scoping.scope_flags(scope).is_top())
                || !scoping.symbol_flags(id).is_variable()
            {
                continue;
            }
            let name = scoping.symbol_name(id);
            let temporary = matches!(name, "acc" | "__abc_this" | "__abc_await" | "__abc_resume")
                || indexed(name, 'v')
                || indexed(name, 'p');
            let captured = scoping
                .get_resolved_references(id)
                .any(|reference| owner(&scoping, reference.scope_id()) != scope);
            locals[id.index()] = temporary && !captured;
        }
        Ok(Self {
            ast: AstBuilder::new(allocator),
            allocator,
            scoping,
            locals,
        })
    }

    fn symbol(&self, identifier: &IdentifierReference<'_>) -> Option<SymbolId> {
        self.scoping
            .get_reference(identifier.reference_id.get()?)
            .symbol_id()
    }

    fn local(&self, identifier: &IdentifierReference<'_>) -> Option<SymbolId> {
        self.symbol(identifier).filter(|id| self.locals[id.index()])
    }

    fn target(&self, target: &AssignmentTarget<'_>) -> Option<SymbolId> {
        if let AssignmentTarget::AssignmentTargetIdentifier(identifier) = target {
            self.local(identifier)
        } else {
            None
        }
    }

    fn atom(&self, expression: &Expression<'a>) -> bool {
        match expression {
            Expression::Identifier(id) => self.local(id).is_some(),
            Expression::BooleanLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::ThisExpression(_)
            | Expression::NewTarget(_) => true,
            Expression::UnaryExpression(expr) => {
                matches!(
                    expr.operator,
                    UnaryOperator::Void | UnaryOperator::UnaryNegation
                ) && matches!(expr.argument, Expression::NumericLiteral(_))
            }
            _ => false,
        }
    }

    fn pure(&self, expression: &Expression<'a>, this_pure: bool) -> bool {
        match expression {
            Expression::Identifier(id) => {
                self.local(id).is_some()
                    || self
                        .symbol(id)
                        .is_some_and(|id| self.scoping.symbol_flags(id).is_function())
            }
            Expression::ThisExpression(_) => this_pure,
            Expression::BooleanLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::NewTarget(_) => true,
            Expression::UnaryExpression(expr) => {
                self.pure(&expr.argument, this_pure)
                    && (matches!(
                        expr.operator,
                        UnaryOperator::Void | UnaryOperator::LogicalNot | UnaryOperator::Typeof
                    ) || (expr.operator == UnaryOperator::UnaryNegation
                        && matches!(expr.argument, Expression::NumericLiteral(_))))
            }
            Expression::BinaryExpression(expr) => {
                matches!(
                    expr.operator,
                    BinaryOperator::StrictEquality | BinaryOperator::StrictInequality
                ) && self.pure(&expr.left, this_pure)
                    && self.pure(&expr.right, this_pure)
            }
            _ => false,
        }
    }
}

fn indexed(name: &str, prefix: char) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

fn owner(scoping: &Scoping, scope: ScopeId) -> ScopeId {
    scoping
        .scope_ancestors(scope)
        .find(|id| scoping.scope_flags(*id).is_var())
        .unwrap_or(scope)
}

type Facts<'a> = HashMap<SymbolId, Expression<'a>>;

// Facts contain only non-captured local aliases and primitives, never property reads or calls.
struct Forward<'c, 'a> {
    context: &'c Context<'a>,
    env: Facts<'a>,
    changed: bool,
}
impl<'a> Forward<'_, 'a> {
    fn kill(&mut self, id: SymbolId) {
        self.env.retain(|key, value| *key != id && !matches!(value, Expression::Identifier(reference) if self.context.symbol(reference) == Some(id)));
    }

    fn remember(&mut self, id: SymbolId, value: &Expression<'a>) {
        self.kill(id);
        if self.context.atom(value)
            && !matches!(value, Expression::Identifier(reference) if self.context.symbol(reference) == Some(id))
        {
            self.env
                .insert(id, value.clone_in_with_semantic_ids(self.context.allocator));
        }
    }

    fn snapshot(&self) -> Facts<'a> {
        self.copy(&self.env)
    }

    fn copy(&self, env: &Facts<'a>) -> Facts<'a> {
        env.iter()
            .map(|(id, expr)| (*id, expr.clone_in_with_semantic_ids(self.context.allocator)))
            .collect()
    }

    fn invalidate(&mut self, statement: &Statement<'a>) {
        let mut writes = Writes {
            context: self.context,
            ids: Vec::new(),
        };
        writes.visit_statement(statement);
        for id in writes.ids {
            self.kill(id);
        }
    }
}

impl<'a> VisitMut<'a> for Forward<'_, 'a> {
    fn visit_function(&mut self, function: &mut Function<'a>, flags: ScopeFlags) {
        let saved = std::mem::take(&mut self.env);
        walk_mut::walk_function(self, function, flags);
        self.env = saved;
    }

    fn visit_arrow_function_expression(&mut self, function: &mut ArrowFunctionExpression<'a>) {
        let saved = std::mem::take(&mut self.env);
        walk_mut::walk_arrow_function_expression(self, function);
        self.env = saved;
    }

    fn visit_class(&mut self, class: &mut Class<'a>) {
        let saved = std::mem::take(&mut self.env);
        walk_mut::walk_class(self, class);
        self.env = saved;
    }

    fn visit_variable_declarator(&mut self, variable: &mut VariableDeclarator<'a>) {
        if let Some(value) = &mut variable.init {
            self.visit_expression(value);
        }
        if let BindingPattern::BindingIdentifier(id) = &variable.id
            && let Some(symbol) = id
                .symbol_id
                .get()
                .filter(|id| self.context.locals[id.index()])
        {
            self.kill(symbol);
            if let Some(value) = &variable.init {
                self.remember(symbol, value);
            }
        }
    }

    fn visit_statement(&mut self, statement: &mut Statement<'a>) {
        match statement {
            Statement::IfStatement(node) => {
                self.visit_expression(&mut node.test);
                if let Some(value) = truth(&node.test) {
                    let chosen = if value {
                        Some(node.consequent.take_in(&self.context.ast))
                    } else {
                        node.alternate.take()
                    };
                    *statement = chosen.unwrap_or_else(|| {
                        Statement::new_empty_statement(node.span, &self.context.ast)
                    });
                    self.changed = true;
                    self.visit_statement(statement);
                    return;
                }
                let saved = self.snapshot();
                self.visit_statement(&mut node.consequent);
                self.env = saved;
                let saved = self.snapshot();
                if let Some(alternate) = &mut node.alternate {
                    self.visit_statement(alternate);
                }
                self.env = saved;
                self.invalidate(statement);
            }
            Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::SwitchStatement(_) => {
                self.invalidate(statement);
                let saved = self.snapshot();
                match statement {
                    Statement::WhileStatement(node) => {
                        self.visit_expression(&mut node.test);
                        self.env = self.copy(&saved);
                        self.visit_statement(&mut node.body);
                    }
                    Statement::DoWhileStatement(node) => {
                        self.visit_statement(&mut node.body);
                        self.env = self.copy(&saved);
                        self.visit_expression(&mut node.test);
                    }
                    Statement::ForStatement(node) => {
                        if let Some(init) = &mut node.init {
                            self.visit_for_statement_init(init);
                        }
                        self.env = self.copy(&saved);
                        if let Some(test) = &mut node.test {
                            self.visit_expression(test);
                        }
                        self.env = self.copy(&saved);
                        self.visit_statement(&mut node.body);
                        self.env = self.copy(&saved);
                        if let Some(update) = &mut node.update {
                            self.visit_expression(update);
                        }
                    }
                    Statement::ForInStatement(node) => {
                        self.visit_expression(&mut node.right);
                        self.env = self.copy(&saved);
                        self.visit_statement(&mut node.body);
                    }
                    Statement::ForOfStatement(node) => {
                        self.visit_expression(&mut node.right);
                        self.env = self.copy(&saved);
                        self.visit_statement(&mut node.body);
                    }
                    Statement::SwitchStatement(node) => {
                        self.visit_expression(&mut node.discriminant);
                        for case in &mut node.cases {
                            self.env = self.copy(&saved);
                            self.visit_switch_case(case);
                        }
                    }
                    _ => unreachable!(),
                }
                self.env = saved;
            }
            Statement::TryStatement(_) => {
                let input = self.snapshot();
                self.invalidate(statement);
                let saved = self.snapshot();
                if let Statement::TryStatement(node) = statement {
                    self.env = input;
                    self.visit_block_statement(&mut node.block);
                    self.env = self.copy(&saved);
                    if let Some(handler) = &mut node.handler {
                        self.visit_catch_clause(handler);
                    }
                    self.env = self.copy(&saved);
                    if let Some(finalizer) = &mut node.finalizer {
                        self.visit_block_statement(finalizer);
                    }
                }
                self.env = saved;
            }
            Statement::LabeledStatement(node) => {
                let saved = self.snapshot();
                self.visit_statement(&mut node.body);
                self.env = saved;
                self.invalidate(statement);
            }
            _ => walk_mut::walk_statement(self, statement),
        }
    }

    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        match expression {
            Expression::Identifier(identifier) => {
                if let Some(id) = self.context.local(identifier)
                    && let Some(value) = self.env.get(&id)
                {
                    *expression = value.clone_in_with_semantic_ids(self.context.allocator);
                    self.changed = true;
                }
                return;
            }
            Expression::AssignmentExpression(assign) => {
                if assign.operator == AssignmentOperator::Assign {
                    self.visit_assignment_target(&mut assign.left);
                    self.visit_expression(&mut assign.right);
                    if let Some(id) = self.context.target(&assign.left) {
                        self.remember(id, &assign.right);
                    }
                } else {
                    walk_mut::walk_assignment_expression(self, assign);
                    self.env.clear();
                }
                return;
            }
            Expression::UpdateExpression(update) => {
                walk_mut::walk_update_expression(self, update);
                self.env.clear();
                return;
            }
            Expression::LogicalExpression(node) => {
                self.visit_expression(&mut node.left);
                let saved = self.snapshot();
                self.visit_expression(&mut node.right);
                self.env = saved;
                let mut writes = Writes {
                    context: self.context,
                    ids: Vec::new(),
                };
                writes.visit_expression(&node.right);
                for id in writes.ids {
                    self.kill(id);
                }
            }
            Expression::ConditionalExpression(node) => {
                self.visit_expression(&mut node.test);
                let saved = self.snapshot();
                self.visit_expression(&mut node.consequent);
                self.env = self.copy(&saved);
                self.visit_expression(&mut node.alternate);
                self.env = saved;
                let mut writes = Writes {
                    context: self.context,
                    ids: Vec::new(),
                };
                writes.visit_expression(expression);
                for id in writes.ids {
                    self.kill(id);
                }
            }
            Expression::AwaitExpression(_) | Expression::YieldExpression(_) => {
                walk_mut::walk_expression(self, expression);
                self.env.clear();
            }
            _ => walk_mut::walk_expression(self, expression),
        }
        if let Some(value) = folded_bool(expression) {
            *expression =
                Expression::new_boolean_literal(expression.span(), value, &self.context.ast);
            self.changed = true;
        }
    }
}

struct Writes<'c, 'a> {
    context: &'c Context<'a>,
    ids: Vec<SymbolId>,
}
impl<'a> Visit<'a> for Writes<'_, 'a> {
    fn visit_identifier_reference(&mut self, id: &IdentifierReference<'a>) {
        if let Some(reference) = id.reference_id.get()
            && self.context.scoping.get_reference(reference).is_write()
            && let Some(symbol) = self.context.local(id)
        {
            self.ids.push(symbol);
        }
    }
    fn visit_variable_declarator(&mut self, variable: &VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(id) = &variable.id
            && let Some(id) = id
                .symbol_id
                .get()
                .filter(|id| self.context.locals[id.index()])
        {
            self.ids.push(id);
        }
        walk::walk_variable_declarator(self, variable);
    }
    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
}

struct Uses<'c, 'a> {
    context: &'c Context<'a>,
    counts: Vec<usize>,
    reads_only: bool,
}
impl<'c, 'a> Uses<'c, 'a> {
    fn new(context: &'c Context<'a>, reads_only: bool) -> Result<Self> {
        let mut counts = Vec::new();
        counts
            .try_reserve_exact(context.locals.len())
            .map_err(|_| resource("ABC AST use allocation failed"))?;
        counts.resize(context.locals.len(), 0);
        Ok(Self {
            context,
            counts,
            reads_only,
        })
    }
}
impl<'a> Visit<'a> for Uses<'_, 'a> {
    fn visit_identifier_reference(&mut self, id: &IdentifierReference<'a>) {
        if let Some(reference) = id.reference_id.get()
            && (!self.reads_only || self.context.scoping.get_reference(reference).is_read())
            && let Some(symbol) = self.context.symbol(id)
        {
            self.counts[symbol.index()] += 1;
        }
    }
}

struct LiveReads<'c, 'a> {
    context: &'c Context<'a>,
    live: &'c mut [bool],
}
impl<'a> Visit<'a> for LiveReads<'_, 'a> {
    fn visit_identifier_reference(&mut self, id: &IdentifierReference<'a>) {
        if let Some(reference) = id.reference_id.get()
            && self.context.scoping.get_reference(reference).is_read()
            && let Some(symbol) = self.context.local(id)
        {
            self.live[symbol.index()] = true;
        }
    }
    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
}

// Catch/finally reads are live on exceptional edges; loops retain conservative live-in sets.
struct Cleanup<'c, 'a> {
    context: &'c Context<'a>,
    reads: &'c [usize],
    this_pure: bool,
    derived_class: bool,
    changed: bool,
}
struct Exit {
    name: String,
    live: Vec<bool>,
}
fn union(live: &mut [bool], other: &[bool]) {
    for (to, from) in live.iter_mut().zip(other) {
        *to |= *from;
    }
}
impl<'a> Cleanup<'_, 'a> {
    fn all_live(&self) -> Vec<bool> {
        self.reads.iter().map(|n| *n != 0).collect()
    }

    fn expression_reads(&self, expression: &Expression<'a>, live: &mut [bool], protected: &[bool]) {
        let mut writes = Writes {
            context: self.context,
            ids: Vec::new(),
        };
        writes.visit_expression(expression);
        if !writes.ids.is_empty() {
            union(live, &self.all_live());
        }
        LiveReads {
            context: self.context,
            live,
        }
        .visit_expression(expression);
        if !self.context.pure(expression, self.this_pure) {
            union(live, protected);
        }
    }

    fn list(
        &mut self,
        statements: &mut ArenaVec<'a, Statement<'a>>,
        mut live: Vec<bool>,
        protected: &[bool],
        exits: &mut Vec<Exit>,
    ) -> Vec<bool> {
        for statement in statements.iter_mut().rev() {
            live = self.statement(statement, live, protected, exits);
        }
        let mut abrupt = false;
        statements.retain(|statement| {
            let remove = matches!(statement, Statement::ExpressionStatement(expr) if self.context.pure(&expr.expression, self.this_pure))
                || matches!(statement, Statement::EmptyStatement(_))
                || matches!(statement, Statement::LabeledStatement(node) if matches!(&node.body, Statement::BlockStatement(block) if block.body.is_empty()))
                || (abrupt && matches!(statement, Statement::BreakStatement(_) | Statement::ContinueStatement(_)));
            abrupt |= matches!(statement, Statement::ReturnStatement(_) | Statement::ThrowStatement(_) | Statement::BreakStatement(_) | Statement::ContinueStatement(_));
            self.changed |= remove;
            !remove
        });
        live
    }

    fn statement(
        &mut self,
        statement: &mut Statement<'a>,
        mut live: Vec<bool>,
        protected: &[bool],
        exits: &mut Vec<Exit>,
    ) -> Vec<bool> {
        match statement {
            Statement::ExpressionStatement(expr) => {
                if let Expression::AssignmentExpression(assign) = &mut expr.expression
                    && assign.operator == AssignmentOperator::Assign
                    && let Some(id) = self.context.target(&assign.left)
                {
                    if matches!(&assign.right, Expression::Identifier(reference) if self.context.local(reference) == Some(id))
                        || (!live[id.index()]
                            && !matches!(assign.right, Expression::StringLiteral(_)))
                    {
                        expr.expression = assign.right.take_in(&self.context.ast);
                        self.changed = true;
                    } else {
                        live[id.index()] = false;
                        self.expression_reads(&assign.right, &mut live, protected);
                        return live;
                    }
                }
                self.expression_reads(&expr.expression, &mut live, protected);
            }
            Statement::BlockStatement(node) => {
                return self.list(&mut node.body, live, protected, exits);
            }
            Statement::IfStatement(node) => {
                let mut left = self.statement(&mut node.consequent, live.clone(), protected, exits);
                let right = node.alternate.as_mut().map_or(live.clone(), |alternate| {
                    self.statement(alternate, live, protected, exits)
                });
                union(&mut left, &right);
                self.expression_reads(&node.test, &mut left, protected);
                return left;
            }
            Statement::LabeledStatement(node) => {
                exits.push(Exit {
                    name: node.label.name.to_string(),
                    live: live.clone(),
                });
                let result = self.statement(&mut node.body, live, protected, exits);
                exits.pop();
                return result;
            }
            Statement::BreakStatement(node) => {
                live = node
                    .label
                    .as_ref()
                    .and_then(|label| {
                        exits
                            .iter()
                            .rev()
                            .find(|exit| exit.name == label.name.as_str())
                    })
                    .map_or_else(|| self.all_live(), |exit| exit.live.clone());
                union(&mut live, protected);
            }
            Statement::ContinueStatement(_) => {
                live = self.all_live();
                union(&mut live, protected);
            }
            Statement::ReturnStatement(node) => {
                live.clone_from_slice(protected);
                if let Some(value) = &node.argument {
                    self.expression_reads(value, &mut live, protected);
                }
            }
            Statement::ThrowStatement(node) => {
                live.clone_from_slice(protected);
                self.expression_reads(&node.argument, &mut live, protected);
            }
            Statement::TryStatement(node) => {
                let mut abrupt = protected.to_vec();
                if let Some(finalizer) = &mut node.finalizer {
                    LiveReads {
                        context: self.context,
                        live: &mut abrupt,
                    }
                    .visit_block_statement(finalizer);
                    live = self.list(&mut finalizer.body, live, protected, exits);
                }
                let mut exceptional = abrupt.clone();
                if let Some(handler) = &mut node.handler {
                    let handler_in =
                        self.list(&mut handler.body.body, live.clone(), &abrupt, exits);
                    union(&mut exceptional, &handler_in);
                }
                return self.list(&mut node.block.body, live, &exceptional, exits);
            }
            Statement::WhileStatement(node) => {
                let mut all = self.all_live();
                union(&mut all, &live);
                union(&mut all, protected);
                self.statement(&mut node.body, all.clone(), protected, exits);
                self.expression_reads(&node.test, &mut all, protected);
                return all;
            }
            Statement::DoWhileStatement(node) => {
                let mut all = self.all_live();
                union(&mut all, &live);
                union(&mut all, protected);
                self.statement(&mut node.body, all.clone(), protected, exits);
                self.expression_reads(&node.test, &mut all, protected);
                return all;
            }
            Statement::VariableDeclaration(declaration) => {
                for variable in declaration.declarations.iter().rev() {
                    if let BindingPattern::BindingIdentifier(id) = &variable.id
                        && let Some(id) = id
                            .symbol_id
                            .get()
                            .filter(|id| self.context.locals[id.index()])
                    {
                        live[id.index()] = false;
                    }
                    if let Some(value) = &variable.init {
                        self.expression_reads(value, &mut live, protected);
                    }
                }
            }
            Statement::FunctionDeclaration(_) | Statement::EmptyStatement(_) => {}
            _ => {
                union(&mut live, &self.all_live());
                union(&mut live, protected);
            }
        }
        live
    }
}
impl<'a> VisitMut<'a> for Cleanup<'_, 'a> {
    fn visit_class(&mut self, class: &mut Class<'a>) {
        let saved = self.derived_class;
        self.derived_class = class.heritage.is_some();
        walk_mut::walk_class(self, class);
        self.derived_class = saved;
    }

    fn visit_program(&mut self, program: &mut Program<'a>) {
        walk_mut::walk_program(self, program);
        self.list(
            &mut program.body,
            vec![false; self.reads.len()],
            &vec![false; self.reads.len()],
            &mut Vec::new(),
        );
    }

    fn visit_function(&mut self, function: &mut Function<'a>, flags: ScopeFlags) {
        let old_this = self.this_pure;
        self.this_pure = !flags.is_constructor() || !self.derived_class;
        walk_mut::walk_function(self, function, flags);
        self.this_pure = old_this;
    }
    fn visit_function_body(&mut self, body: &mut FunctionBody<'a>) {
        walk_mut::walk_function_body(self, body);
        self.list(
            &mut body.statements,
            vec![false; self.reads.len()],
            &vec![false; self.reads.len()],
            &mut Vec::new(),
        );
    }
}

struct Declarations<'c, 'a> {
    context: &'c Context<'a>,
    uses: &'c [usize],
    this_pure: bool,
    derived_class: bool,
    changed: bool,
}
impl<'a> VisitMut<'a> for Declarations<'_, 'a> {
    fn visit_class(&mut self, class: &mut Class<'a>) {
        let saved = self.derived_class;
        self.derived_class = class.heritage.is_some();
        walk_mut::walk_class(self, class);
        self.derived_class = saved;
    }

    fn visit_function(&mut self, function: &mut Function<'a>, flags: ScopeFlags) {
        let saved = self.this_pure;
        self.this_pure = !flags.is_constructor() || !self.derived_class;
        walk_mut::walk_function(self, function, flags);
        self.this_pure = saved;
    }
    fn visit_variable_declaration(&mut self, declaration: &mut VariableDeclaration<'a>) {
        walk_mut::walk_variable_declaration(self, declaration);
        declaration.declarations.retain(|variable| {
            let remove = if let BindingPattern::BindingIdentifier(id) = &variable.id {
                id.symbol_id
                    .get()
                    .is_some_and(|id| self.context.locals[id.index()] && self.uses[id.index()] == 0)
                    && variable
                        .init
                        .as_ref()
                        .is_none_or(|value| self.context.pure(value, self.this_pure))
            } else {
                false
            };
            self.changed |= remove;
            !remove
        });
    }
    fn visit_statements(&mut self, statements: &mut ArenaVec<'a, Statement<'a>>) {
        walk_mut::walk_statements(self, statements);
        statements.retain(|statement| !matches!(statement, Statement::VariableDeclaration(decl) if decl.declarations.is_empty()));
    }
}

#[derive(PartialEq)]
enum Primitive {
    Bool(bool),
    Number(f64),
    Null,
    Undefined,
}
fn primitive(expression: &Expression<'_>) -> Option<Primitive> {
    match expression {
        Expression::BooleanLiteral(value) => Some(Primitive::Bool(value.value)),
        Expression::NumericLiteral(value) => Some(Primitive::Number(value.value)),
        Expression::NullLiteral(_) => Some(Primitive::Null),
        Expression::UnaryExpression(expr)
            if expr.operator == UnaryOperator::Void
                && matches!(expr.argument, Expression::NumericLiteral(_)) =>
        {
            Some(Primitive::Undefined)
        }
        _ => None,
    }
}
fn truth(expression: &Expression<'_>) -> Option<bool> {
    Some(match primitive(expression)? {
        Primitive::Bool(value) => value,
        Primitive::Number(value) => value != 0.0 && !value.is_nan(),
        Primitive::Null | Primitive::Undefined => false,
    })
}
fn folded_bool(expression: &Expression<'_>) -> Option<bool> {
    match expression {
        Expression::UnaryExpression(expr) if expr.operator == UnaryOperator::LogicalNot => {
            truth(&expr.argument).map(|x| !x)
        }
        Expression::LogicalExpression(expr) => {
            let (Expression::BooleanLiteral(left), Expression::BooleanLiteral(right)) =
                (&expr.left, &expr.right)
            else {
                return None;
            };
            match expr.operator {
                LogicalOperator::Or => Some(left.value || right.value),
                LogicalOperator::And => Some(left.value && right.value),
                _ => None,
            }
        }
        Expression::BinaryExpression(expr) => {
            let left = primitive(&expr.left)?;
            let right = primitive(&expr.right)?;
            match expr.operator {
                BinaryOperator::StrictEquality => Some(left == right),
                BinaryOperator::StrictInequality => Some(left != right),
                BinaryOperator::Equality | BinaryOperator::Inequality => {
                    if std::mem::discriminant(&left) != std::mem::discriminant(&right) {
                        return None;
                    }
                    Some((left == right) == (expr.operator == BinaryOperator::Equality))
                }
                _ => None,
            }
        }
        _ => None,
    }
}
