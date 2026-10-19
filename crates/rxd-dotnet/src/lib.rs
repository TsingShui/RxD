//! Static .NET metadata, original IL and conservative method reconstruction.
mod class;
mod decompile;
mod il;
mod input;
mod metadata;
mod model;
#[cfg(feature = "csharp-decompiler")]
mod native;
mod refs;
mod scan;
mod signature;
mod strings;
mod validation;

pub use decompile::{render, render_class};
pub use input::Input;
pub use model::*;

pub use refs::{Reference, ReferenceResult, SemanticTarget, TargetResolver};

pub use scan::{IlScan, TokenSite};
