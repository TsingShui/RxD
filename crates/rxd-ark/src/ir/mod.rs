mod async_frame;
mod classes;
mod intrinsic;
pub(crate) use intrinsic::Intrinsic;
pub(crate) mod lift;

use crate::{Code, Method, Name, cfg::Graph};
use oxc_syntax::operator::{BinaryOperator, UnaryOperator};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ValueId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    Acc,
    Resume,
    Register(u32),
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Effects {
    pub may_throw: bool,
    pub observable: bool,
    pub suspend: bool,
}

impl Effects {
    pub const PURE: Self = Self {
        may_throw: false,
        observable: false,
        suspend: false,
    };
    pub const READ: Self = Self {
        may_throw: true,
        observable: true,
        suspend: false,
    };
    pub const WRITE: Self = Self::READ;
    pub const CALL: Self = Self::READ;
    pub const SUSPEND: Self = Self {
        may_throw: true,
        observable: true,
        suspend: true,
    };
    pub fn ordered(self) -> bool {
        self.observable || self.may_throw || self.suspend
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Constant {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(Arc<Name>),
    Hole,
}

#[derive(Clone, Debug)]
pub(crate) enum Key {
    Name(Arc<Name>),
    Index(u32),
    Value(ValueId),
}

#[derive(Clone, Debug)]
pub(crate) enum Op {
    Constant(Constant),
    Read(Slot),
    This,
    NewTarget,
    Function,
    Global(Arc<Name>),
    GlobalObject,
    Unary(UnaryOperator, ValueId),
    Binary(BinaryOperator, ValueId, ValueId),
    BranchCompare(BinaryOperator, ValueId, ValueId),
    ToNumeric(ValueId),
    Update(ValueId, bool),
    Get(ValueId, Key),
    Call {
        callee: ValueId,
        receiver: Option<ValueId>,
        args: Vec<ValueId>,
    },
    Construct {
        callee: ValueId,
        args: Vec<ValueId>,
    },
    Array(Vec<ValueId>),
    Object(Vec<(Key, ValueId)>),
    LoadCell {
        depth: u32,
        index: u32,
    },
    Environment,
    Home,
    Import(u32),
    Module(u32),
    Namespace(u32),
    Closure {
        method: u32,
        length: Option<u32>,
        environment: Option<ValueId>,
    },
    Await(ValueId),
    Iterator(ValueId),
    Keys(ValueId),
    Next(ValueId),
    DynamicImport(ValueId),
    Delete(ValueId, ValueId),
    CopyData(ValueId, ValueId),
    RestArgs(u32),
    ArraySpread(ValueId, ValueId, ValueId),
    Class {
        constructor: u32,
        name: Arc<Name>,
        base: ValueId,
        environment: Option<ValueId>,
        length: u32,
        members: Vec<classes::ClassMember>,
    },
    Method {
        method: u32,
        name: Arc<Name>,
        length: u32,
        home: ValueId,
    },
    SuperCall {
        function: Option<ValueId>,
        args: Vec<ValueId>,
        spread: Option<ValueId>,
    },
    SuperGet(ValueId, Key),
    Apply(ValueId, ValueId, ValueId),
    Generator,
    Suspend(ValueId, ValueId),
    AsyncYield(ValueId, ValueId, ValueId),
    GeneratorResume(ValueId, bool),
    IterResult(ValueId, ValueId),
    TypeError(&'static str),
    NamedError(&'static str, &'static str, Option<ValueId>),
    Intrinsic(Intrinsic, Vec<ValueId>),
}

impl Op {
    pub fn warning(&self) -> Option<crate::ReconstructionWarning> {
        use crate::ReconstructionWarning as W;
        match self {
            Self::Intrinsic(kind, _) => Some(kind.warning()),
            Self::BranchCompare(..) => Some(W::AssumedBranchSemantics),
            Self::Iterator(_) => Some(W::IteratorRuntimeSemantics),
            Self::Keys(_) | Self::Next(_) => Some(W::PropertyEnumeration),
            Self::ArraySpread(..) => Some(W::ArraySpread),
            Self::Apply(..)
            | Self::SuperCall {
                spread: Some(_), ..
            } => Some(W::SpreadCall),
            Self::AsyncYield(..) => Some(W::AsyncFrameApproximation),
            Self::Generator | Self::Suspend(..) | Self::GeneratorResume(..) => {
                Some(W::GeneratorReconstruction)
            }
            Self::Home
            | Self::Class { .. }
            | Self::Method { .. }
            | Self::SuperCall { .. }
            | Self::SuperGet(..) => Some(W::ClassReconstruction),
            _ => None,
        }
    }
    pub fn inputs(&self) -> Vec<ValueId> {
        match self {
            Self::Unary(_, value)
            | Self::ToNumeric(value)
            | Self::Update(value, _)
            | Self::Await(value)
            | Self::Iterator(value)
            | Self::Keys(value)
            | Self::Next(value)
            | Self::DynamicImport(value) => vec![*value],
            Self::NamedError(_, _, value) => value.iter().copied().collect(),
            Self::Closure { environment, .. } => environment.iter().copied().collect(),
            Self::Class {
                base,
                environment,
                members,
                ..
            } => {
                let mut values = vec![*base];
                values.extend(environment);
                for member in members {
                    values.extend(member.value);
                    if let Key::Value(key) = member.name {
                        values.push(key);
                    }
                }
                values
            }
            Self::Method { home, .. } => vec![*home],
            Self::SuperCall {
                function,
                args,
                spread,
            } => function.iter().chain(args).chain(spread).copied().collect(),
            Self::ArraySpread(dst, index, src)
            | Self::Apply(dst, index, src)
            | Self::AsyncYield(dst, index, src) => {
                vec![*dst, *index, *src]
            }
            Self::Suspend(generator, value) | Self::IterResult(generator, value) => {
                vec![*generator, *value]
            }
            Self::GeneratorResume(generator, _) => vec![*generator],
            Self::Binary(_, left, right)
            | Self::BranchCompare(_, left, right)
            | Self::Delete(left, right)
            | Self::CopyData(left, right) => vec![*left, *right],
            Self::Get(object, key) | Self::SuperGet(object, key) => {
                let mut ids = vec![*object];
                if let Key::Value(value) = key {
                    ids.push(*value);
                }
                ids
            }
            Self::Call {
                callee,
                receiver,
                args,
            } => {
                let mut ids = vec![*callee];
                ids.extend(receiver);
                ids.extend(args);
                ids
            }
            Self::Construct { callee, args } => {
                let mut ids = vec![*callee];
                ids.extend(args);
                ids
            }
            Self::Array(values) | Self::Intrinsic(_, values) => values.clone(),
            Self::Object(entries) => entries
                .iter()
                .flat_map(|(key, value)| {
                    let mut ids = Vec::new();
                    if let Key::Value(value) = key {
                        ids.push(*value);
                    }
                    ids.push(*value);
                    ids
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

pub(crate) struct Value {
    pub pc: u32,
    pub block: usize,
    pub op: Op,
    pub effects: Effects,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreKind {
    Assign,
    Define,
    ClassSensitive,
}

#[derive(Clone)]
pub(crate) enum Action {
    Write(Slot, ValueId),
    Set {
        object: ValueId,
        key: Key,
        value: ValueId,
        own: StoreKind,
    },
    StoreCell {
        depth: u32,
        index: u32,
        value: ValueId,
    },
    StoreModule(u32, ValueId),
    PushEnv(u32),
    PopEnv,
    CheckHole(ValueId, Arc<Name>),
    CheckObject(ValueId),
    CheckHoleValue(ValueId, ValueId),
    Debugger,
    CheckSuper(ValueId, bool),
    Accessors {
        object: ValueId,
        key: ValueId,
        getter: ValueId,
        setter: ValueId,
        set_name: ValueId,
    },
    SuperSet {
        receiver: ValueId,
        key: Key,
        value: ValueId,
    },
}

impl Action {
    pub fn inputs(&self) -> Vec<ValueId> {
        match self {
            Self::Write(_, value)
            | Self::StoreCell { value, .. }
            | Self::StoreModule(_, value)
            | Self::CheckHole(value, _) => vec![*value],
            Self::CheckObject(value) => vec![*value, *value, *value],
            Self::CheckHoleValue(value, name) => vec![*value, *name],
            Self::CheckSuper(value, _) => vec![*value, *value],
            Self::Accessors {
                object,
                key,
                getter,
                setter,
                set_name,
            } => vec![*object, *key, *getter, *setter, *set_name],
            Self::SuperSet {
                receiver,
                key,
                value,
            } => {
                let mut values = vec![*receiver, *value];
                if let Key::Value(key) = key {
                    values.push(*key);
                }
                values
            }
            Self::Set {
                object, key, value, ..
            } => {
                let mut ids = vec![*object, *value];
                if let Key::Value(value) = key {
                    ids.push(*value);
                }
                ids
            }
            _ => Vec::new(),
        }
    }
    pub fn effects(&self) -> Effects {
        match self {
            Self::Write(..) => Effects::PURE,
            _ => Effects::WRITE,
        }
    }
}

pub(crate) enum Event {
    Define(ValueId),
    Action { pc: u32, action: Action },
}

pub(crate) enum Terminator {
    Jump(usize),
    Branch {
        value: ValueId,
        nonzero: bool,
        yes: usize,
        no: usize,
    },
    Return(Option<ValueId>),
    Throw(ValueId),
}

impl Terminator {
    pub fn inputs(&self) -> Vec<ValueId> {
        match self {
            Self::Branch { value, .. } | Self::Throw(value) => vec![*value],
            Self::Return(Some(value)) => vec![*value],
            _ => Vec::new(),
        }
    }
    pub fn successors(&self) -> Vec<usize> {
        match self {
            Self::Jump(target) => vec![*target],
            Self::Branch { yes, no, .. } if yes != no => vec![*yes, *no],
            Self::Branch { yes, .. } => vec![*yes],
            _ => Vec::new(),
        }
    }
}

pub(crate) struct Block {
    pub pc: u32,
    pub events: Vec<Event>,
    pub end: Terminator,
    pub handlers: Vec<usize>,
}

pub(crate) struct Frame {
    pub locals: u32,
    pub params: u32,
    pub func: Option<u32>,
    pub new_target: Option<u32>,
    pub this_reg: Option<u32>,
    pub first_param: u32,
    pub arrow: bool,
    pub asynchronous: bool,
    pub constructor: bool,
    pub generator: bool,
    pub async_generator: bool,
    pub multi_generator: bool,
    pub raw_async: bool,
    pub frame_warning: bool,
    pub typed_catches: bool,
    pub residual_only: bool,
}

pub(crate) struct Function<'a> {
    pub method: Method,
    pub code: Code<'a>,
    pub frame: Frame,
    pub graph: Graph,
    pub blocks: Vec<Block>,
    pub values: Vec<Value>,
}

impl Function<'_> {
    pub fn boolean(&self, value: ValueId) -> bool {
        matches!(
            self.values[value.0].op,
            Op::BranchCompare(..)
                | Op::Unary(UnaryOperator::LogicalNot, _)
                | Op::Binary(
                    BinaryOperator::Equality
                        | BinaryOperator::Inequality
                        | BinaryOperator::StrictEquality
                        | BinaryOperator::StrictInequality
                        | BinaryOperator::LessThan
                        | BinaryOperator::LessEqualThan
                        | BinaryOperator::GreaterThan
                        | BinaryOperator::GreaterEqualThan
                        | BinaryOperator::Instanceof
                        | BinaryOperator::In,
                    _,
                    _
                )
                | Op::Constant(Constant::Bool(_))
        )
    }
}
