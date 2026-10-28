//! Invocation-local guards for the legacy, Result-less recovery helpers.
//!
//! Only this private cancellation payload is converted at `limited`'s boundary.
//! `resume_unwind` does not invoke or replace the process panic hook. Other
//! panics retain their payload and continue unwinding after restoring the scope.
use crate::{ast::*, object::PyObject, PycError, Result};
use std::{
    cell::{Cell, RefCell},
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    rc::Rc,
};

#[derive(Clone, Copy)]
pub struct Limits {
    pub work: Option<usize>,
    pub ast_depth: usize,
}
#[derive(Default, Clone, Copy)]
pub struct Usage {
    pub work: usize,
    pub ast_depth: usize,
}
#[derive(Clone, Copy)]
struct Snapshot {
    active: bool,
    work: usize,
    work_limit: usize,
    ast_depth: usize,
    max_depth: usize,
    frames: usize,
    frame_limit: usize,
}
struct Cancel(&'static str);
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static WORK: Cell<usize> = const { Cell::new(0) };
    static WORK_LIMIT: Cell<usize> = const { Cell::new(usize::MAX) };
    static AST_DEPTH: Cell<usize> = const { Cell::new(0) };
    static MAX_DEPTH: Cell<usize> = const { Cell::new(0) };
    static FRAMES: Cell<usize> = const { Cell::new(0) };
    static FRAME_LIMIT: Cell<usize> = const { Cell::new(0) };
}

fn snapshot() -> Snapshot {
    Snapshot {
        active: ACTIVE.with(Cell::get),
        work: WORK.with(Cell::get),
        work_limit: WORK_LIMIT.with(Cell::get),
        ast_depth: AST_DEPTH.with(Cell::get),
        max_depth: MAX_DEPTH.with(Cell::get),
        frames: FRAMES.with(Cell::get),
        frame_limit: FRAME_LIMIT.with(Cell::get),
    }
}

fn restore(snapshot: Snapshot) {
    WORK.with(|cell| cell.set(snapshot.work));
    WORK_LIMIT.with(|cell| cell.set(snapshot.work_limit));
    AST_DEPTH.with(|cell| cell.set(snapshot.ast_depth));
    MAX_DEPTH.with(|cell| cell.set(snapshot.max_depth));
    FRAMES.with(|cell| cell.set(snapshot.frames));
    FRAME_LIMIT.with(|cell| cell.set(snapshot.frame_limit));
    ACTIVE.with(|cell| cell.set(snapshot.active));
}

pub fn cancelled<T>(action: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(value) => value,
        Err(payload) => match payload.downcast::<Cancel>() {
            Ok(cancel) => Err(PycError::ResourceLimit(cancel.0)),
            Err(payload) => resume_unwind(payload),
        },
    }
}

pub fn limited<T>(limits: Limits, action: impl FnOnce() -> Result<T>) -> (Result<T>, Usage) {
    let previous = snapshot();
    WORK.with(|cell| cell.set(0));
    WORK_LIMIT.with(|cell| cell.set(limits.work.unwrap_or(usize::MAX)));
    AST_DEPTH.with(|cell| cell.set(0));
    MAX_DEPTH.with(|cell| cell.set(limits.ast_depth));
    FRAMES.with(|cell| cell.set(0));
    FRAME_LIMIT.with(|cell| cell.set(limits.ast_depth.saturating_mul(4).saturating_add(32)));
    ACTIVE.with(|cell| cell.set(true));
    let result = catch_unwind(AssertUnwindSafe(action));
    let usage = Usage {
        work: WORK.with(Cell::get),
        ast_depth: AST_DEPTH.with(Cell::get),
    };
    restore(previous);
    let result = match result {
        Ok(value) => value,
        Err(payload) => match payload.downcast::<Cancel>() {
            Ok(cancel) => Err(PycError::ResourceLimit(cancel.0)),
            Err(payload) => resume_unwind(payload),
        },
    };
    (result, usage)
}

pub fn charge(units: usize) {
    if !ACTIVE.with(Cell::get) {
        return;
    }
    let limit = WORK_LIMIT.with(Cell::get);
    let used = WORK.with(Cell::get);
    if units > limit.saturating_sub(used) {
        resume_unwind(Box::new(Cancel("decompile work budget exceeded")));
    }
    let Some(next) = used.checked_add(units) else {
        resume_unwind(Box::new(Cancel("decompile work budget exceeded")));
    };
    WORK.with(|cell| cell.set(next));
}

fn depth(depth: usize) {
    if !ACTIVE.with(Cell::get) {
        return;
    }
    let limit = MAX_DEPTH.with(Cell::get);
    let used = AST_DEPTH.with(Cell::get);
    if depth > limit {
        if limit > used {
            AST_DEPTH.with(|cell| cell.set(limit));
        }
        resume_unwind(Box::new(Cancel(
            "instruction-derived AST depth budget exceeded",
        )));
    }
    if depth > used {
        AST_DEPTH.with(|cell| cell.set(depth));
    }
}

pub struct Frame(bool);
impl Drop for Frame {
    fn drop(&mut self) {
        if self.0 {
            FRAMES.with(|cell| cell.set(cell.get().saturating_sub(1)));
        }
    }
}
/// Bound recursive helper entry as well as AST structure. The constant factor
/// accommodates fixed helper frames between adjacent AST nodes; it is finite.
pub fn enter() -> Frame {
    if !ACTIVE.with(Cell::get) {
        return Frame(false);
    }
    let limit = WORK_LIMIT.with(Cell::get);
    let used = WORK.with(Cell::get);
    if 1 > limit.saturating_sub(used) {
        resume_unwind(Box::new(Cancel("decompile work budget exceeded")));
    }
    let Some(next) = used.checked_add(1) else {
        resume_unwind(Box::new(Cancel("decompile work budget exceeded")));
    };
    WORK.with(|cell| cell.set(next));
    let max = FRAME_LIMIT.with(Cell::get);
    let frames = FRAMES.with(Cell::get);
    if frames >= max {
        resume_unwind(Box::new(Cancel(
            "instruction-derived recovery nesting budget exceeded",
        )));
    }
    FRAMES.with(|cell| cell.set(frames + 1));
    Frame(true)
}
/// Charge expansion before allocating, not after processing the new vector.
pub fn capacity<T>(n: usize) -> Vec<T> {
    charge(n);
    Vec::with_capacity(n)
}
pub fn to_vec<T: Clone>(v: &[T]) -> Vec<T> {
    charge(v.len());
    v.to_vec()
}
pub fn filled<T: Clone>(value: T, n: usize) -> Vec<T> {
    charge(n);
    vec![value; n]
}
pub fn repeat(s: &str, n: usize) -> String {
    let bytes = s.len().checked_mul(n).unwrap_or(usize::MAX);
    charge(bytes);
    s.repeat(n)
}
pub trait Node {
    fn check(&self);
}
impl Node for Expr {
    fn check(&self) {
        walk(Item::Expr(self));
    }
}
impl Node for FunctionDef {
    fn check(&self) {
        walk(Item::Function(self));
    }
}
impl Node for PyObject {
    fn check(&self) {
        charge(1);
    }
}
pub fn rc<T: Node>(node: T) -> Rc<T> {
    // The node is folded into a statement, which is checked before it joins
    // the body. Walking every intermediate wrapper would repeatedly traverse
    // the same subtree while the expression is built bottom-up.
    charge(1);
    Rc::new(node)
}
pub fn pattern(pattern: &Pattern) {
    walk(Item::Pattern(pattern));
}
pub fn statement(stmt: &Stmt) {
    walk(Item::Stmt(stmt));
}
pub fn statements(stmts: &[Stmt]) {
    for stmt in stmts {
        statement(stmt);
    }
}

/// Check emitted ASTs without letting internal stack markers become source names.
pub(crate) fn source_ready(stmts: &[Stmt]) -> bool {
    let mut ready = true;
    for stmt in stmts {
        ready &= walk(Item::Stmt(stmt));
    }
    ready
}

enum Item<'a> {
    Expr(&'a Expr),
    Stmt(&'a Stmt),
    Pattern(&'a Pattern),
    Params(&'a Parameters),
    Function(&'a FunctionDef),
    TypeParam(&'a TypeParam),
    FString(&'a FString),
}
fn walk(root: Item<'_>) -> bool {
    // No recursive calls: even rejected instruction-built chains are inspected
    // without consuming the native stack. Every scheduled edge is charged.
    let mut ready = true;
    let mut todo = vec![(root, 1)];
    let mut work = 0usize;
    let mut max_level = 1usize;
    while let Some((item, level)) = todo.pop() {
        work = work.saturating_add(1);
        max_level = max_level.max(level);
        let mut edges = 0usize;
        let mut add = |item| {
            edges = edges.saturating_add(1);
            todo.push((item, level + 1));
        };
        match item {
            Item::Expr(e) => match e {
                Expr::Const(_) => {}
                Expr::Name(name) => {
                    work = work.saturating_add(name.len());
                    ready &= !name.as_bytes().contains(&0);
                }
                Expr::Attribute { value, .. }
                | Expr::Unary { operand: value, .. }
                | Expr::Starred(value)
                | Expr::Await(value)
                | Expr::YieldFrom(value)
                | Expr::Backquote(value) => add(Item::Expr(value)),
                Expr::Subscript { value, index }
                | Expr::Binary {
                    left: value,
                    right: index,
                    ..
                }
                | Expr::Named {
                    target: value,
                    value: index,
                } => {
                    add(Item::Expr(value));
                    add(Item::Expr(index));
                }
                Expr::Slice(s) => {
                    for e in [&s.start, &s.stop, &s.step].into_iter().flatten() {
                        add(Item::Expr(e));
                    }
                }
                Expr::Compare { operands, .. } => {
                    for e in operands {
                        add(Item::Expr(e));
                    }
                }
                Expr::BoolOp { values, .. }
                | Expr::Tuple(values)
                | Expr::List(values)
                | Expr::Set(values) => {
                    for e in values {
                        add(Item::Expr(e));
                    }
                }
                Expr::Dict(items) => {
                    for (k, v) in items {
                        add(Item::Expr(k));
                        add(Item::Expr(v));
                    }
                }
                Expr::Call {
                    func,
                    args,
                    keywords,
                    star_args,
                    star_kwargs,
                } => {
                    add(Item::Expr(func));
                    for e in args {
                        add(Item::Expr(e));
                    }
                    for (_, e) in keywords {
                        add(Item::Expr(e));
                    }
                    for e in [star_args, star_kwargs].into_iter().flatten() {
                        add(Item::Expr(e));
                    }
                }
                Expr::Comprehension {
                    elt,
                    key,
                    generators,
                    ..
                } => {
                    add(Item::Expr(elt));
                    if let Some(e) = key {
                        add(Item::Expr(e));
                    }
                    for g in generators {
                        add(Item::Expr(&g.target));
                        add(Item::Expr(&g.iter));
                        for e in &g.ifs {
                            add(Item::Expr(e));
                        }
                    }
                }
                Expr::Lambda { params, body } => {
                    add(Item::Params(params));
                    add(Item::Expr(body));
                }
                Expr::Function(f) => add(Item::Function(f)),
                Expr::Ternary {
                    cond,
                    then_expr,
                    else_expr,
                } => {
                    for e in [cond, then_expr, else_expr] {
                        add(Item::Expr(e));
                    }
                }
                Expr::Yield(e) => {
                    if let Some(e) = e {
                        add(Item::Expr(e));
                    }
                }
                Expr::FString(s) | Expr::TString(s) => add(Item::FString(s)),
                Expr::TypeParamNode(p) => add(Item::TypeParam(p)),
                Expr::GenericBase { params, .. } => {
                    for p in params {
                        add(Item::TypeParam(p));
                    }
                }
                Expr::TypeAliasValue {
                    type_params, value, ..
                } => {
                    add(Item::Expr(value));
                    for p in type_params {
                        add(Item::TypeParam(p));
                    }
                }
            },
            Item::Params(p) => {
                for a in p
                    .args
                    .iter()
                    .chain(&p.kwonly)
                    .chain(p.vararg.iter())
                    .chain(p.kwarg.iter())
                {
                    if let Some(e) = &a.annotation {
                        add(Item::Expr(e));
                    }
                }
                for e in &p.defaults {
                    add(Item::Expr(e));
                }
                for e in p
                    .kw_defaults
                    .iter()
                    .flatten()
                    .chain(p.returns_annotation.iter())
                {
                    add(Item::Expr(e));
                }
            }
            Item::Function(f) => {
                add(Item::Params(&f.params));
                for e in f.decorators.iter().chain(f.returns.iter()) {
                    add(Item::Expr(e));
                }
                for p in &f.type_params {
                    add(Item::TypeParam(p));
                }
            }
            Item::TypeParam(p) => match p {
                TypeParam::TypeVar { bound, default, .. } => {
                    for e in [bound, default].into_iter().flatten() {
                        add(Item::Expr(e));
                    }
                }
                TypeParam::ParamSpec { default, .. } | TypeParam::TypeVarTuple { default, .. } => {
                    if let Some(e) = default {
                        add(Item::Expr(e));
                    }
                }
            },
            Item::FString(s) => {
                for p in &s.parts {
                    charge(1);
                    if let FStringPart::Value {
                        value, format_spec, ..
                    } = p
                    {
                        add(Item::Expr(value));
                        if let Some(s) = format_spec {
                            add(Item::FString(s));
                        }
                    }
                }
            }
            Item::Pattern(p) => match p {
                Pattern::Value(e) => add(Item::Expr(e)),
                Pattern::Capture(_) | Pattern::Wildcard => {}
                Pattern::Or(ps) => {
                    for p in ps {
                        add(Item::Pattern(p));
                    }
                }
                Pattern::Sequence { items, star } => {
                    for p in items {
                        add(Item::Pattern(p));
                    }
                    if let Some((p, _)) = star {
                        add(Item::Pattern(p));
                    }
                }
                Pattern::Mapping { items, .. } => {
                    for (e, p) in items {
                        add(Item::Expr(e));
                        add(Item::Pattern(p));
                    }
                }
                Pattern::Class {
                    cls,
                    patterns,
                    keywords,
                } => {
                    add(Item::Expr(cls));
                    for p in patterns {
                        add(Item::Pattern(p));
                    }
                    for (_, p) in keywords {
                        add(Item::Pattern(p));
                    }
                }
            },
            Item::Stmt(s) => {
                // Schedule expression and statement children explicitly. The
                // nonrecursive walk charges repeated shared-reference paths.
                match s {
                    Stmt::Expr(e) => add(Item::Expr(e)),
                    Stmt::Assign { targets, value, .. } => {
                        for e in targets {
                            add(Item::Expr(e));
                        }
                        add(Item::Expr(value));
                    }
                    Stmt::AugAssign { target, value, .. } => {
                        add(Item::Expr(target));
                        add(Item::Expr(value));
                    }
                    Stmt::AnnAssign {
                        target,
                        annotation,
                        value,
                    } => {
                        add(Item::Expr(target));
                        add(Item::Expr(annotation));
                        if let Some(e) = value {
                            add(Item::Expr(e));
                        }
                    }
                    Stmt::Print { dest, values, .. } => {
                        if let Some(e) = dest {
                            add(Item::Expr(e));
                        }
                        for e in values {
                            add(Item::Expr(e));
                        }
                    }
                    Stmt::Exec {
                        code,
                        globals,
                        locals,
                    } => {
                        add(Item::Expr(code));
                        for e in [globals, locals].into_iter().flatten() {
                            add(Item::Expr(e));
                        }
                    }
                    Stmt::Return(e) => {
                        if let Some(e) = e {
                            add(Item::Expr(e));
                        }
                    }
                    Stmt::Delete(es) => {
                        for e in es {
                            add(Item::Expr(e));
                        }
                    }
                    Stmt::If { cond, body, orelse } | Stmt::While { cond, body, orelse } => {
                        add(Item::Expr(cond));
                        for st in body.iter().chain(orelse) {
                            add(Item::Stmt(st));
                        }
                    }
                    Stmt::For {
                        target,
                        iter,
                        body,
                        orelse,
                        ..
                    } => {
                        add(Item::Expr(target));
                        add(Item::Expr(iter));
                        for st in body.iter().chain(orelse) {
                            add(Item::Stmt(st));
                        }
                    }
                    Stmt::Try {
                        body,
                        handlers,
                        orelse,
                        finalbody,
                    } => {
                        for st in body.iter().chain(orelse).chain(finalbody) {
                            add(Item::Stmt(st));
                        }
                        for h in handlers {
                            for e in [&h.type_, &h.name].into_iter().flatten() {
                                add(Item::Expr(e));
                            }
                            for st in &h.body {
                                add(Item::Stmt(st));
                            }
                        }
                    }
                    Stmt::With { items, body, .. } => {
                        for i in items {
                            add(Item::Expr(&i.ctx));
                            if let Some(e) = &i.target {
                                add(Item::Expr(e));
                            }
                        }
                        for st in body {
                            add(Item::Stmt(st));
                        }
                    }
                    Stmt::Raise {
                        exc,
                        cause,
                        py2_inst,
                        py2_tb,
                    } => {
                        for e in [exc, cause, py2_inst, py2_tb].into_iter().flatten() {
                            add(Item::Expr(e));
                        }
                    }
                    Stmt::Assert { test, msg } => {
                        add(Item::Expr(test));
                        if let Some(e) = msg {
                            add(Item::Expr(e));
                        }
                    }
                    Stmt::FuncDef(f, body) => {
                        add(Item::Function(f));
                        for st in body {
                            add(Item::Stmt(st));
                        }
                    }
                    Stmt::ClassDef {
                        bases,
                        keywords,
                        star_args,
                        star_kwargs,
                        decorators,
                        body,
                        type_params,
                        ..
                    } => {
                        for e in bases.iter().chain(decorators) {
                            add(Item::Expr(e));
                        }
                        for (_, e) in keywords {
                            add(Item::Expr(e));
                        }
                        for e in [star_args, star_kwargs].into_iter().flatten() {
                            add(Item::Expr(e));
                        }
                        for st in body {
                            add(Item::Stmt(st));
                        }
                        for p in type_params {
                            add(Item::TypeParam(p));
                        }
                    }
                    Stmt::TypeAlias {
                        type_params, value, ..
                    } => {
                        add(Item::Expr(value));
                        for p in type_params {
                            add(Item::TypeParam(p));
                        }
                    }
                    Stmt::Match { subject, cases } => {
                        add(Item::Expr(subject));
                        for c in cases {
                            add(Item::Pattern(&c.pattern));
                            if let Some(e) = &c.guard {
                                add(Item::Expr(e));
                            }
                            for st in &c.body {
                                add(Item::Stmt(st));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        drop(add);
        work = work.saturating_add(edges);
    }
    charge(work);
    depth(max_level);
    ready
}

// Autoref fallback charges one shallow clone; concrete collection/string
// receivers charge their allocation size before Clone executes. AST values
// additionally validate/charge the complete expansion before derived Clone.
pub struct CopySize<T>(pub T);
pub trait CopyWork {
    fn copy_work(self) -> usize;
}
impl<T> CopyWork for &CopySize<T> {
    fn copy_work(self) -> usize {
        1
    }
}
impl<T> CopyWork for CopySize<&Vec<T>> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
impl<T> CopyWork for CopySize<&&Vec<T>> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
impl CopyWork for CopySize<&String> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
impl CopyWork for CopySize<&&String> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
impl CopyWork for CopySize<&Expr> {
    fn copy_work(self) -> usize {
        charge(1);
        1
    }
}
impl CopyWork for CopySize<&&Expr> {
    fn copy_work(self) -> usize {
        charge(1);
        1
    }
}
impl CopyWork for CopySize<&Stmt> {
    fn copy_work(self) -> usize {
        charge(1);
        1
    }
}
impl CopyWork for CopySize<&&Stmt> {
    fn copy_work(self) -> usize {
        charge(1);
        1
    }
}
impl CopyWork for CopySize<&FunctionDef> {
    fn copy_work(self) -> usize {
        self.0.check();
        1
    }
}
impl CopyWork for CopySize<&&FunctionDef> {
    fn copy_work(self) -> usize {
        self.0.check();
        1
    }
}
#[macro_export]
macro_rules! budget_clone {
    ($value:expr) => {{
        use $crate::budget::CopyWork as _;
        let value = &$value;
        $crate::budget::charge($crate::budget::CopySize(value).copy_work());
        (*value).clone()
    }};
}

impl CopyWork for CopySize<&str> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
impl CopyWork for CopySize<&&str> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
impl<T> CopyWork for CopySize<&[T]> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
impl<T> CopyWork for CopySize<&&[T]> {
    fn copy_work(self) -> usize {
        self.0.len()
    }
}
#[macro_export]
macro_rules! budget_string {
    ($value:expr) => {{
        use $crate::budget::CopyWork as _;
        let value = &$value;
        $crate::budget::charge($crate::budget::CopySize(value).copy_work());
        value.to_string()
    }};
}
#[macro_export]
macro_rules! budget_owned {
    ($value:expr) => {{
        use $crate::budget::CopyWork as _;
        let value = &$value;
        $crate::budget::charge($crate::budget::CopySize(value).copy_work());
        (*value).to_owned()
    }};
}

/// Preserve owned moves while guarding borrowed identifier allocation.
pub trait IntoName {
    fn into_name(self) -> String;
}
impl IntoName for String {
    fn into_name(self) -> String {
        self
    }
}
impl IntoName for &str {
    fn into_name(self) -> String {
        charge(self.len());
        self.to_owned()
    }
}
impl IntoName for &String {
    fn into_name(self) -> String {
        charge(self.len());
        self.to_owned()
    }
}

/// Guard each formatted byte before the formatter grows its temporary string.
pub fn format(arguments: std::fmt::Arguments<'_>) -> String {
    struct Writer(String);
    impl std::fmt::Write for Writer {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            push_str(&mut self.0, text);
            Ok(())
        }
    }
    let mut writer = Writer(String::new());
    std::fmt::write(&mut writer, arguments).expect("string formatting failed");
    writer.0
}
#[macro_export]
macro_rules! budget_format {
    ($($argument:tt)*) => { $crate::budget::format(format_args!($($argument)*)) };
}
pub fn push_str(output: &mut String, text: &str) {
    charge(text.len());
    output.push_str(text);
}
pub fn push(output: &mut String, ch: char) {
    charge(ch.len_utf8());
    output.push(ch);
}
pub fn join(parts: &[String], separator: &str) -> String {
    charge(parts.len());
    let bytes = parts.iter().fold(
        separator
            .len()
            .saturating_mul(parts.len().saturating_sub(1)),
        |bytes, part| bytes.saturating_add(part.len()),
    );
    charge(bytes);
    parts.join(separator)
}
/// Determine replacement expansion without allocating, then charge both the
/// second UTF-8 scan and the exact owned result before `from_utf8_lossy`.
pub fn utf8_lossy(bytes: &[u8]) -> String {
    charge(bytes.len());
    let mut remaining = bytes;
    let mut output_bytes = 0usize;
    while !remaining.is_empty() {
        charge(1);
        match std::str::from_utf8(remaining) {
            Ok(text) => {
                output_bytes = output_bytes.saturating_add(text.len());
                break;
            }
            Err(error) => {
                output_bytes = output_bytes
                    .saturating_add(error.valid_up_to())
                    .saturating_add(3);
                let invalid = error
                    .error_len()
                    .unwrap_or(remaining.len() - error.valid_up_to());
                remaining = &remaining[error.valid_up_to() + invalid..];
            }
        }
    }
    charge(bytes.len().saturating_add(output_bytes));
    String::from_utf8_lossy(bytes).into_owned()
}
