//! Version-agnostic abstract syntax tree for decompiled Python code.
//!
//! The decompiler builds this tree from any version's bytecode; the code
//! generator renders it back to source using version-specific syntax rules.

use std::rc::Rc;

use crate::code::CodeObject;
use crate::object::ObjectRef;

#[derive(Clone)]
pub struct ExprRef(Rc<ExprNode>);
struct ExprNode {
    value: Expr,
    summary: crate::budget::Summary,
}
impl Clone for ExprNode {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            summary: crate::budget::Summary::default(),
        }
    }
}
impl ExprRef {
    pub(crate) fn new(value: Expr, summary: crate::budget::Summary) -> Self {
        Self(Rc::new(ExprNode { value, summary }))
    }
    pub fn ptr_eq(left: &Self, right: &Self) -> bool {
        Rc::ptr_eq(&left.0, &right.0)
    }
    pub fn make_mut(&mut self) -> &mut Expr {
        let node = Rc::make_mut(&mut self.0);
        node.summary = crate::budget::Summary::default();
        &mut node.value
    }
    pub(crate) fn summary(&self) -> Option<crate::budget::Summary> {
        self.0.summary.valid()
    }
}
impl std::ops::Deref for ExprRef {
    type Target = Expr;
    fn deref(&self) -> &Expr {
        &self.0.value
    }
}
impl AsRef<Expr> for ExprRef {
    fn as_ref(&self) -> &Expr {
        self
    }
}
impl std::fmt::Debug for ExprRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.value.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,    // -x
    Pos,    // +x
    Invert, // ~x
    Not,    // not x
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mult,
    Div,
    FloorDiv,
    Mod,
    Pow,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
    MatMult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    LtE,
    Eq,
    NotEq,
    Gt,
    GtE,
    In,
    NotIn,
    Is,
    IsNot,
    /// py2 `except A, e:` matching, rendered specially by codegen.
    ExceptionMatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOpKind {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompKind {
    List,
    Set,
    Dict,
    Generator,
}

#[derive(Debug)]
pub struct Comprehension {
    pub target: ExprRef,
    pub iter: ExprRef,
    pub ifs: Vec<ExprRef>,
    pub is_async: bool,
    /// 3.12-only: the filter's `if` clause sat on its own source line —
    /// the compiler emits the PJIF->shared-backedge filter shape there
    /// (same-line filters compile to PJIT+continue), so the renderer
    /// must reproduce the line break for sig-exactness
    pub if_line_break: bool,
}

#[derive(Debug)]
pub struct Parameters {
    pub args: Vec<Param>,
    pub vararg: Option<Param>,
    pub kwonly: Vec<Param>,
    pub kwarg: Option<Param>,
    /// Defaults align to the tail of `args`.
    pub defaults: Vec<ExprRef>,
    pub kw_defaults: Vec<Option<ExprRef>>,
    /// posonlyargcount (3.8+)
    pub posonly_count: usize,
    /// return annotation stored during MAKE_FUNCTION processing
    pub returns_annotation: Option<ExprRef>,
}

#[derive(Debug)]
pub struct Param {
    pub name: String,
    pub annotation: Option<ExprRef>,
}

impl Parameters {
    pub fn empty() -> Parameters {
        Parameters {
            args: Vec::new(),
            vararg: None,
            kwonly: Vec::new(),
            kwarg: None,
            defaults: Vec::new(),
            kw_defaults: Vec::new(),
            posonly_count: 0,
            returns_annotation: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
            && self.vararg.is_none()
            && self.kwonly.is_empty()
            && self.kwarg.is_none()
    }
}

/// One f-string piece: literal text or a formatted replacement field.
#[derive(Debug)]
pub enum FStringPart {
    Literal(String),
    Value {
        value: ExprRef,
        /// 'r', 's', 'a' or none
        conversion: Option<char>,
        format_spec: Option<Box<FString>>,
    },
}

#[derive(Debug)]
pub struct FString {
    pub parts: Vec<FStringPart>,
}

#[derive(Debug)]
pub struct SliceExpr {
    pub start: Option<ExprRef>,
    pub stop: Option<ExprRef>,
    pub step: Option<ExprRef>,
}

#[derive(Debug)]
pub enum Expr {
    /// A marshalled constant (int, str, tuple of constants, ...).
    Const(ObjectRef),
    Name(String),
    Attribute {
        value: ExprRef,
        attr: String,
    },
    Subscript {
        value: ExprRef,
        index: ExprRef,
    },
    Slice(Box<SliceExpr>),
    Unary {
        op: UnaryOp,
        operand: ExprRef,
    },
    Binary {
        op: BinaryOp,
        left: ExprRef,
        right: ExprRef,
    },
    /// Chained comparisons: `a op0 b op1 c` -> operands.len() == ops.len()+1
    Compare {
        operands: Vec<ExprRef>,
        ops: Vec<CmpOp>,
    },
    BoolOp {
        op: BoolOpKind,
        values: Vec<ExprRef>,
    },
    Call {
        func: ExprRef,
        args: Vec<ExprRef>,
        keywords: Vec<(Option<String>, ExprRef)>,
        /// `*args` expression (CALL_FUNCTION_EX / py2 VAR forms)
        star_args: Option<ExprRef>,
        /// `**kwargs` expression
        star_kwargs: Option<ExprRef>,
    },
    Tuple(Vec<ExprRef>),
    List(Vec<ExprRef>),
    Set(Vec<ExprRef>),
    Dict(Vec<(ExprRef, ExprRef)>),
    Starred(ExprRef),
    Comprehension {
        kind: CompKind,
        elt: ExprRef,
        /// key expression for dict comprehensions
        key: Option<ExprRef>,
        generators: Vec<Comprehension>,
    },
    Lambda {
        params: Box<Parameters>,
        body: ExprRef,
    },
    /// A function object (code + signature) before it is bound by a def
    /// statement or used as a value.
    Function(Rc<FunctionDef>),
    Ternary {
        cond: ExprRef,
        then_expr: ExprRef,
        else_expr: ExprRef,
    },
    Await(ExprRef),
    /// 3.8+ walrus: `(target := value)`
    Named {
        target: ExprRef,
        value: ExprRef,
    },
    Yield(Option<ExprRef>),
    YieldFrom(ExprRef),
    FString(Box<FString>),
    /// 3.14+ template string (PEP 750 t-string): `t"..."`. Reuses the
    /// FString part model (Literal text + Value interpolations); the
    /// `t` prefix and Template semantics distinguish it from FString.
    TString(Box<FString>),
    /// py2 backquote repr: `x`
    Backquote(ExprRef),
    /// PEP 695: symbolic type-parameter value living only inside the
    /// `<generic parameters of X>` wrapper evaluation; never codegen'd.
    TypeParamNode(Box<TypeParam>),
    /// PEP 695: the compiler-generated `C[T]` generic base artifact of a
    /// `class C[T](...)` wrapper; filtered out of the rendered bases.
    GenericBase {
        name: String,
        params: Vec<TypeParam>,
    },
    /// PEP 695: result of the TYPEALIAS intrinsic; the store path turns it
    /// into `Stmt::TypeAlias`.
    TypeAliasValue {
        name: String,
        type_params: Vec<TypeParam>,
        value: ExprRef,
    },
}

/// PEP 695/696 type parameter (`def f[T]`, `class C[T: bound]`, `type X[T = d]`)
#[derive(Debug)]
pub enum TypeParam {
    TypeVar {
        name: String,
        bound: Option<ExprRef>,
        default: Option<ExprRef>,
    },
    ParamSpec {
        name: String,
        default: Option<ExprRef>,
    },
    TypeVarTuple {
        name: String,
        default: Option<ExprRef>,
    },
}

#[derive(Debug)]
pub struct FunctionDef {
    /// def name (from the binding target, since code.name may be mangled)
    pub name: String,
    pub code: Rc<CodeObject>,
    pub params: Parameters,
    pub decorators: Vec<ExprRef>,
    /// return annotation (`-> X`)
    pub returns: Option<ExprRef>,
    pub is_async: bool,
    /// 3.12+ PEP 695 type parameters (`def f[T](...)`)
    pub type_params: Vec<TypeParam>,
}

#[derive(Debug)]
pub struct ExceptHandler {
    /// None = bare `except:`
    pub type_: Option<ExprRef>,
    pub name: Option<ExprRef>,
    pub body: Vec<Stmt>,
    /// 3.11+ except* (ExceptionGroup clause)
    pub is_star: bool,
}

/// 3.10+ `match` statement patterns
#[derive(Debug)]
pub enum Pattern {
    /// literal or dotted-name value pattern (`case 3:`, `case Color.RED:`)
    Value(ExprRef),
    /// capture pattern (`case x:`)
    Capture(String),
    /// wildcard (`case _:`)
    Wildcard,
    /// `case p1 | p2:`
    Or(Vec<Pattern>),
    /// `case [a, b]` / `case (a, b)`; `star` is the `*rest` element with
    /// the number of items that follow it
    Sequence {
        items: Vec<Pattern>,
        star: Option<(Box<Pattern>, usize)>,
    },
    /// `case {'k': v, **rest}`
    Mapping {
        items: Vec<(ExprRef, Pattern)>,
        rest: Option<String>,
    },
    /// `case Point(x, y=q)`
    Class {
        cls: ExprRef,
        patterns: Vec<Pattern>,
        keywords: Vec<(String, Pattern)>,
    },
}

#[derive(Debug)]
pub struct MatchCase {
    pub pattern: Pattern,
    pub guard: Option<ExprRef>,
    pub body: Vec<Stmt>,
}

#[derive(Debug)]
pub struct WithItem {
    pub ctx: ExprRef,
    pub target: Option<ExprRef>,
}

#[derive(Debug)]
pub enum Stmt {
    Expr(ExprRef),
    Assign {
        targets: Vec<ExprRef>,
        value: ExprRef,
        /// source line-wrap positions inside a tuple-unpack target:
        /// break the rendered line BEFORE these element indices so the
        /// recompile reproduces the original per-line STORE fusion
        /// (3.13+ STORE_FAST_STORE_FAST only fuses same-line stores;
        /// abc _dump_registry's 4-target unpack wrapped across two
        /// lines in the source)
        wrap_before: Vec<u16>,
    },
    AugAssign {
        target: ExprRef,
        op: BinaryOp,
        value: ExprRef,
    },
    AnnAssign {
        target: ExprRef,
        annotation: ExprRef,
        value: Option<ExprRef>,
    },
    /// py2 `print` statement
    Print {
        dest: Option<ExprRef>,
        values: Vec<ExprRef>,
        newline: bool,
    },
    /// py2 `exec` statement
    Exec {
        code: ExprRef,
        globals: Option<ExprRef>,
        locals: Option<ExprRef>,
    },
    Return(Option<ExprRef>),
    Delete(Vec<ExprRef>),
    Pass,
    Break,
    Continue,
    Import {
        /// (module, asname)
        names: Vec<(String, Option<String>)>,
    },
    ImportFrom {
        module: String,
        level: u32,
        /// (name, asname); single ("*", None) for star imports
        names: Vec<(String, Option<String>)>,
    },
    Global(Vec<String>),
    Nonlocal(Vec<String>),
    If {
        cond: ExprRef,
        body: Vec<Stmt>,
        orelse: Vec<Stmt>,
    },
    While {
        cond: ExprRef,
        body: Vec<Stmt>,
        orelse: Vec<Stmt>,
    },
    For {
        target: ExprRef,
        iter: ExprRef,
        body: Vec<Stmt>,
        orelse: Vec<Stmt>,
        is_async: bool,
    },
    Try {
        body: Vec<Stmt>,
        handlers: Vec<ExceptHandler>,
        orelse: Vec<Stmt>,
        finalbody: Vec<Stmt>,
    },
    With {
        items: Vec<WithItem>,
        body: Vec<Stmt>,
        is_async: bool,
    },
    Raise {
        exc: Option<ExprRef>,
        cause: Option<ExprRef>,
        /// py2 `raise T, I[, B]`: instance and traceback slots (None on py3)
        py2_inst: Option<ExprRef>,
        py2_tb: Option<ExprRef>,
    },
    Assert {
        test: ExprRef,
        msg: Option<ExprRef>,
    },
    FuncDef(Rc<FunctionDef>, Vec<Stmt>),
    ClassDef {
        coordinate: Option<usize>,
        name: String,
        bases: Vec<ExprRef>,
        keywords: Vec<(Option<String>, ExprRef)>,
        star_args: Option<ExprRef>,
        star_kwargs: Option<ExprRef>,
        decorators: Vec<ExprRef>,
        body: Vec<Stmt>,
        /// 3.12+ PEP 695 type parameters (`class C[T]:`)
        type_params: Vec<TypeParam>,
    },
    /// 3.12+ PEP 695 type alias: `type X[T] = value`
    TypeAlias {
        name: String,
        type_params: Vec<TypeParam>,
        value: ExprRef,
    },
    /// 3.10+ `match subject: case ...:`
    Match {
        subject: ExprRef,
        cases: Vec<MatchCase>,
    },
    /// Fallback for bytecode we could not model; keeps the disassembly text
    /// so output stays informative (graceful degradation).
    Unimplemented(String),
    /// Whole-code fallback; the module renderer owns the original bytes.
    BytecodeFallback(usize),
}

// Budget-aware cloning retains the upstream AST shape.
impl Clone for Pattern {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        crate::budget::pattern(self);
        match self {
            Self::Value(field_0) => Self::Value(crate::budget_clone!(field_0)),
            Self::Capture(field_0) => Self::Capture(crate::budget_clone!(field_0)),
            Self::Wildcard => Self::Wildcard,
            Self::Or(field_0) => Self::Or(crate::budget_clone!(field_0)),
            Self::Sequence { items, star } => Self::Sequence {
                items: crate::budget_clone!(items),
                star: crate::budget_clone!(star),
            },
            Self::Mapping { items, rest } => Self::Mapping {
                items: crate::budget_clone!(items),
                rest: crate::budget_clone!(rest),
            },
            Self::Class {
                cls,
                patterns,
                keywords,
            } => Self::Class {
                cls: crate::budget_clone!(cls),
                patterns: crate::budget_clone!(patterns),
                keywords: crate::budget_clone!(keywords),
            },
        }
    }
}

// Budget-aware cloning retains the upstream AST shape.
impl Clone for Stmt {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        match self {
            Self::Expr(field_0) => Self::Expr(crate::budget_clone!(field_0)),
            Self::Assign {
                targets,
                value,
                wrap_before,
            } => Self::Assign {
                targets: crate::budget_clone!(targets),
                value: crate::budget_clone!(value),
                wrap_before: crate::budget_clone!(wrap_before),
            },
            Self::AugAssign { target, op, value } => Self::AugAssign {
                target: crate::budget_clone!(target),
                op: crate::budget_clone!(op),
                value: crate::budget_clone!(value),
            },
            Self::AnnAssign {
                target,
                annotation,
                value,
            } => Self::AnnAssign {
                target: crate::budget_clone!(target),
                annotation: crate::budget_clone!(annotation),
                value: crate::budget_clone!(value),
            },
            Self::Print {
                dest,
                values,
                newline,
            } => Self::Print {
                dest: crate::budget_clone!(dest),
                values: crate::budget_clone!(values),
                newline: crate::budget_clone!(newline),
            },
            Self::Exec {
                code,
                globals,
                locals,
            } => Self::Exec {
                code: crate::budget_clone!(code),
                globals: crate::budget_clone!(globals),
                locals: crate::budget_clone!(locals),
            },
            Self::Return(field_0) => Self::Return(crate::budget_clone!(field_0)),
            Self::Delete(field_0) => Self::Delete(crate::budget_clone!(field_0)),
            Self::Pass => Self::Pass,
            Self::Break => Self::Break,
            Self::Continue => Self::Continue,
            Self::Import { names } => Self::Import {
                names: crate::budget_clone!(names),
            },
            Self::ImportFrom {
                module,
                level,
                names,
            } => Self::ImportFrom {
                module: crate::budget_clone!(module),
                level: crate::budget_clone!(level),
                names: crate::budget_clone!(names),
            },
            Self::Global(field_0) => Self::Global(crate::budget_clone!(field_0)),
            Self::Nonlocal(field_0) => Self::Nonlocal(crate::budget_clone!(field_0)),
            Self::If { cond, body, orelse } => Self::If {
                cond: crate::budget_clone!(cond),
                body: crate::budget_clone!(body),
                orelse: crate::budget_clone!(orelse),
            },
            Self::While { cond, body, orelse } => Self::While {
                cond: crate::budget_clone!(cond),
                body: crate::budget_clone!(body),
                orelse: crate::budget_clone!(orelse),
            },
            Self::For {
                target,
                iter,
                body,
                orelse,
                is_async,
            } => Self::For {
                target: crate::budget_clone!(target),
                iter: crate::budget_clone!(iter),
                body: crate::budget_clone!(body),
                orelse: crate::budget_clone!(orelse),
                is_async: crate::budget_clone!(is_async),
            },
            Self::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => Self::Try {
                body: crate::budget_clone!(body),
                handlers: crate::budget_clone!(handlers),
                orelse: crate::budget_clone!(orelse),
                finalbody: crate::budget_clone!(finalbody),
            },
            Self::With {
                items,
                body,
                is_async,
            } => Self::With {
                items: crate::budget_clone!(items),
                body: crate::budget_clone!(body),
                is_async: crate::budget_clone!(is_async),
            },
            Self::Raise {
                exc,
                cause,
                py2_inst,
                py2_tb,
            } => Self::Raise {
                exc: crate::budget_clone!(exc),
                cause: crate::budget_clone!(cause),
                py2_inst: crate::budget_clone!(py2_inst),
                py2_tb: crate::budget_clone!(py2_tb),
            },
            Self::Assert { test, msg } => Self::Assert {
                test: crate::budget_clone!(test),
                msg: crate::budget_clone!(msg),
            },
            Self::FuncDef(field_0, field_1) => {
                Self::FuncDef(crate::budget_clone!(field_0), crate::budget_clone!(field_1))
            }
            Self::ClassDef {
                coordinate,
                name,
                bases,
                keywords,
                star_args,
                star_kwargs,
                decorators,
                body,
                type_params,
            } => Self::ClassDef {
                coordinate: *coordinate,
                name: crate::budget_clone!(name),
                bases: crate::budget_clone!(bases),
                keywords: crate::budget_clone!(keywords),
                star_args: crate::budget_clone!(star_args),
                star_kwargs: crate::budget_clone!(star_kwargs),
                decorators: crate::budget_clone!(decorators),
                body: crate::budget_clone!(body),
                type_params: crate::budget_clone!(type_params),
            },
            Self::TypeAlias {
                name,
                type_params,
                value,
            } => Self::TypeAlias {
                name: crate::budget_clone!(name),
                type_params: crate::budget_clone!(type_params),
                value: crate::budget_clone!(value),
            },
            Self::Match { subject, cases } => Self::Match {
                subject: crate::budget_clone!(subject),
                cases: crate::budget_clone!(cases),
            },
            Self::Unimplemented(field_0) => Self::Unimplemented(crate::budget_clone!(field_0)),
            Self::BytecodeFallback(index) => Self::BytecodeFallback(*index),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for Comprehension {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            target: crate::budget_clone!(self.target),
            iter: crate::budget_clone!(self.iter),
            ifs: crate::budget_clone!(self.ifs),
            is_async: crate::budget_clone!(self.is_async),
            if_line_break: crate::budget_clone!(self.if_line_break),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for Parameters {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            args: crate::budget_clone!(self.args),
            vararg: crate::budget_clone!(self.vararg),
            kwonly: crate::budget_clone!(self.kwonly),
            kwarg: crate::budget_clone!(self.kwarg),
            defaults: crate::budget_clone!(self.defaults),
            kw_defaults: crate::budget_clone!(self.kw_defaults),
            posonly_count: crate::budget_clone!(self.posonly_count),
            returns_annotation: crate::budget_clone!(self.returns_annotation),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for Param {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            name: crate::budget_clone!(self.name),
            annotation: crate::budget_clone!(self.annotation),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for FStringPart {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        match self {
            Self::Literal(field_0) => Self::Literal(crate::budget_clone!(field_0)),
            Self::Value {
                value,
                conversion,
                format_spec,
            } => Self::Value {
                value: crate::budget_clone!(value),
                conversion: crate::budget_clone!(conversion),
                format_spec: crate::budget_clone!(format_spec),
            },
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for FString {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            parts: crate::budget_clone!(self.parts),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for SliceExpr {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            start: crate::budget_clone!(self.start),
            stop: crate::budget_clone!(self.stop),
            step: crate::budget_clone!(self.step),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for Expr {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        match self {
            Self::Const(field_0) => Self::Const(crate::budget_clone!(field_0)),
            Self::Name(field_0) => Self::Name(crate::budget_clone!(field_0)),
            Self::Attribute { value, attr } => Self::Attribute {
                value: crate::budget_clone!(value),
                attr: crate::budget_clone!(attr),
            },
            Self::Subscript { value, index } => Self::Subscript {
                value: crate::budget_clone!(value),
                index: crate::budget_clone!(index),
            },
            Self::Slice(field_0) => Self::Slice(crate::budget_clone!(field_0)),
            Self::Unary { op, operand } => Self::Unary {
                op: crate::budget_clone!(op),
                operand: crate::budget_clone!(operand),
            },
            Self::Binary { op, left, right } => Self::Binary {
                op: crate::budget_clone!(op),
                left: crate::budget_clone!(left),
                right: crate::budget_clone!(right),
            },
            Self::Compare { operands, ops } => Self::Compare {
                operands: crate::budget_clone!(operands),
                ops: crate::budget_clone!(ops),
            },
            Self::BoolOp { op, values } => Self::BoolOp {
                op: crate::budget_clone!(op),
                values: crate::budget_clone!(values),
            },
            Self::Call {
                func,
                args,
                keywords,
                star_args,
                star_kwargs,
            } => Self::Call {
                func: crate::budget_clone!(func),
                args: crate::budget_clone!(args),
                keywords: crate::budget_clone!(keywords),
                star_args: crate::budget_clone!(star_args),
                star_kwargs: crate::budget_clone!(star_kwargs),
            },
            Self::Tuple(field_0) => Self::Tuple(crate::budget_clone!(field_0)),
            Self::List(field_0) => Self::List(crate::budget_clone!(field_0)),
            Self::Set(field_0) => Self::Set(crate::budget_clone!(field_0)),
            Self::Dict(field_0) => Self::Dict(crate::budget_clone!(field_0)),
            Self::Starred(field_0) => Self::Starred(crate::budget_clone!(field_0)),
            Self::Comprehension {
                kind,
                elt,
                key,
                generators,
            } => Self::Comprehension {
                kind: crate::budget_clone!(kind),
                elt: crate::budget_clone!(elt),
                key: crate::budget_clone!(key),
                generators: crate::budget_clone!(generators),
            },
            Self::Lambda { params, body } => Self::Lambda {
                params: crate::budget_clone!(params),
                body: crate::budget_clone!(body),
            },
            Self::Function(field_0) => Self::Function(crate::budget_clone!(field_0)),
            Self::Ternary {
                cond,
                then_expr,
                else_expr,
            } => Self::Ternary {
                cond: crate::budget_clone!(cond),
                then_expr: crate::budget_clone!(then_expr),
                else_expr: crate::budget_clone!(else_expr),
            },
            Self::Await(field_0) => Self::Await(crate::budget_clone!(field_0)),
            Self::Named { target, value } => Self::Named {
                target: crate::budget_clone!(target),
                value: crate::budget_clone!(value),
            },
            Self::Yield(field_0) => Self::Yield(crate::budget_clone!(field_0)),
            Self::YieldFrom(field_0) => Self::YieldFrom(crate::budget_clone!(field_0)),
            Self::FString(field_0) => Self::FString(crate::budget_clone!(field_0)),
            Self::TString(field_0) => Self::TString(crate::budget_clone!(field_0)),
            Self::Backquote(field_0) => Self::Backquote(crate::budget_clone!(field_0)),
            Self::TypeParamNode(field_0) => Self::TypeParamNode(crate::budget_clone!(field_0)),
            Self::GenericBase { name, params } => Self::GenericBase {
                name: crate::budget_clone!(name),
                params: crate::budget_clone!(params),
            },
            Self::TypeAliasValue {
                name,
                type_params,
                value,
            } => Self::TypeAliasValue {
                name: crate::budget_clone!(name),
                type_params: crate::budget_clone!(type_params),
                value: crate::budget_clone!(value),
            },
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for TypeParam {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        match self {
            Self::TypeVar {
                name,
                bound,
                default,
            } => Self::TypeVar {
                name: crate::budget_clone!(name),
                bound: crate::budget_clone!(bound),
                default: crate::budget_clone!(default),
            },
            Self::ParamSpec { name, default } => Self::ParamSpec {
                name: crate::budget_clone!(name),
                default: crate::budget_clone!(default),
            },
            Self::TypeVarTuple { name, default } => Self::TypeVarTuple {
                name: crate::budget_clone!(name),
                default: crate::budget_clone!(default),
            },
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for FunctionDef {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            name: crate::budget_clone!(self.name),
            code: crate::budget_clone!(self.code),
            params: crate::budget_clone!(self.params),
            decorators: crate::budget_clone!(self.decorators),
            returns: crate::budget_clone!(self.returns),
            is_async: crate::budget_clone!(self.is_async),
            type_params: crate::budget_clone!(self.type_params),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for ExceptHandler {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            type_: crate::budget_clone!(self.type_),
            name: crate::budget_clone!(self.name),
            body: crate::budget_clone!(self.body),
            is_star: crate::budget_clone!(self.is_star),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for MatchCase {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            pattern: crate::budget_clone!(self.pattern),
            guard: crate::budget_clone!(self.guard),
            body: crate::budget_clone!(self.body),
        }
    }
}

// Guard field copies before derived-structure expansion.
impl Clone for WithItem {
    fn clone(&self) -> Self {
        let _budget_frame = crate::budget::enter();
        Self {
            ctx: crate::budget_clone!(self.ctx),
            target: crate::budget_clone!(self.target),
        }
    }
}
