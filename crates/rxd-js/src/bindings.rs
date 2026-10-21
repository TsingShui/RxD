//! Conservative local const-string evidence. No writes, object reads or scope crossing.
use oxc_ast::{
    AstKind,
    ast::{Expression, IdentifierReference, VariableDeclarationKind},
};
use oxc_semantic::{Semantic, SymbolId};
use oxc_span::{GetSpan, Span};
use oxc_syntax::scope::ScopeId;
use rxd_core::{Limits, Result, perf::Perf};
use std::collections::HashMap;

struct Constant<'a> {
    initializer: &'a Expression<'a>,
    binding: Span,
    scope: ScopeId,
}
pub(crate) struct Bindings<'a> {
    semantic: Semantic<'a>,
    constants: HashMap<SymbolId, Constant<'a>>,
    dynamic: bool,
}
impl<'a> Bindings<'a> {
    pub fn build(
        program: &'a oxc_ast::ast::Program<'a>,
        limits: Limits,
        perf: &mut Perf,
    ) -> Result<Self> {
        let semantic = crate::lexical::build(program, limits, perf)?;
        let dynamic = crate::lexical::is_dynamic(&semantic);
        let mut constants = HashMap::new();
        if !dynamic {
            for node in semantic.nodes().iter() {
                let AstKind::VariableDeclarator(d) = node.kind() else {
                    continue;
                };
                let AstKind::VariableDeclaration(parent) = semantic.nodes().parent_kind(node.id())
                else {
                    continue;
                };
                if parent.kind != VariableDeclarationKind::Const {
                    continue;
                }
                let (Some(id), Some(initializer)) =
                    (d.id.get_binding_identifier(), d.init.as_ref())
                else {
                    continue;
                };
                let Some(symbol) = id.symbol_id.get() else {
                    continue;
                };
                if semantic
                    .scoping()
                    .get_resolved_references(symbol)
                    .any(|r| r.is_write())
                {
                    continue;
                }
                constants.insert(
                    symbol,
                    Constant {
                        initializer,
                        binding: id.span,
                        scope: node.scope_id(),
                    },
                );
            }
        }
        Ok(Self {
            semantic,
            constants,
            dynamic,
        })
    }
    pub fn initializer(
        &self,
        id: &IdentifierReference<'a>,
    ) -> std::result::Result<(&'a Expression<'a>, Span), &'static str> {
        if self.dynamic {
            return Err("dynamic_scope");
        }
        let reference = self
            .semantic
            .scoping()
            .get_reference(id.reference_id.get().ok_or("binding_unavailable")?);
        let symbol = reference.symbol_id().ok_or("binding_unavailable")?;
        let c = self.constants.get(&symbol).ok_or("not_local_const")?;
        if c.scope != reference.scope_id() {
            return Err("binding_scope_boundary");
        }
        if c.initializer.span().end > id.span.start {
            return Err("binding_before_initializer");
        }
        Ok((c.initializer, c.binding))
    }
}
