use crate::ReconstructionWarning as W;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Intrinsic {
    Accessor,
    NamedCall,
    ObjectLiteral,
    AsyncEnter,
    AsyncAwait,
    AsyncResolve,
    AsyncReject,
    AsyncGenReject,
    AsyncPause,
    GeneratorState,
    SendableClass,
    SendableEnv,
    SendableGet,
    SendableSet,
    SendableLoadClass,
    SendableImport,
    SendableLocal,
    LazyImport,
    ConcurrentResult,
    BigInt,
    RegExp,
    Arguments,
    Symbol,
    PropertyKey,
    SetPrototype,
    ObjectRest,
    ConstructSpread,
    IteratorNext,
    CloseIterator,
    AsyncIterator,
    Template,
    PrivateGet,
    PrivateSet,
    PrivateDefine,
    PrivateHas,
    PrivateKeys,
    GlobalGet,
    GlobalSet,
    GlobalDeclare,
    PatchGet,
    PatchSet,
    OwnNamed,
}

impl Intrinsic {
    pub fn name(self) -> &'static str {
        match self {
            Self::Accessor => "accessor",
            Self::NamedCall => "named_call",
            Self::ObjectLiteral => "object_literal",
            Self::AsyncEnter => "async_enter",
            Self::AsyncAwait => "async_await",
            Self::AsyncResolve => "async_resolve",
            Self::AsyncReject => "async_reject",
            Self::AsyncGenReject => "async_gen_reject",
            Self::AsyncPause => "async_pause",
            Self::GeneratorState => "generator_state_set",
            Self::SendableClass => "sendable_class",
            Self::SendableEnv => "sendable_env",
            Self::SendableGet => "sendable_get",
            Self::SendableSet => "sendable_set",
            Self::SendableLoadClass => "sendable_load_class",
            Self::SendableImport => "sendable_import",
            Self::SendableLocal => "sendable_local",
            Self::LazyImport => "lazy_import",
            Self::ConcurrentResult => "concurrent_result",
            Self::BigInt => "bigint",
            Self::RegExp => "regexp",
            Self::Arguments => "arguments",
            Self::Symbol => "symbol",
            Self::PropertyKey => "property_key",
            Self::SetPrototype => "set_prototype",
            Self::ObjectRest => "object_rest",
            Self::ConstructSpread => "construct_spread",
            Self::IteratorNext => "iterator_next",
            Self::CloseIterator => "close_iterator",
            Self::AsyncIterator => "async_iterator",
            Self::Template => "template",
            Self::PrivateGet => "private_get",
            Self::PrivateSet => "private_set",
            Self::PrivateDefine => "private_define",
            Self::PrivateHas => "private_has",
            Self::PrivateKeys => "private_keys",
            Self::GlobalGet => "global_get",
            Self::GlobalSet => "global_set",
            Self::GlobalDeclare => "global_declare",
            Self::PatchGet => "patch_get",
            Self::PatchSet => "patch_set",
            Self::OwnNamed => "own_named",
        }
    }
    pub fn warning(self) -> W {
        match self {
            Self::AsyncEnter
            | Self::AsyncAwait
            | Self::AsyncResolve
            | Self::AsyncReject
            | Self::AsyncGenReject
            | Self::AsyncPause
            | Self::GeneratorState => W::AsyncFrameApproximation,
            Self::SendableClass
            | Self::SendableEnv
            | Self::SendableGet
            | Self::SendableSet
            | Self::SendableLoadClass
            | Self::SendableImport
            | Self::ConcurrentResult => W::SendableSemantics,
            Self::SendableLocal => W::SendableLocalModule,
            Self::LazyImport => W::LazyModuleTiming,
            Self::PrivateGet
            | Self::PrivateSet
            | Self::PrivateDefine
            | Self::PrivateHas
            | Self::PrivateKeys => W::PrivateProperties,
            Self::GlobalGet
            | Self::GlobalSet
            | Self::GlobalDeclare
            | Self::PatchGet
            | Self::PatchSet => W::RuntimeBindings,
            Self::ConstructSpread => W::SpreadCall,
            Self::AsyncIterator | Self::IteratorNext | Self::CloseIterator => W::IteratorOperations,
            _ => W::JavaScriptIntrinsics,
        }
    }
}
