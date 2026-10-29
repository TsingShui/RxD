//! Static .NET metadata, original IL and conservative method reconstruction.
mod binary;
mod bundle;
mod class;
mod decompile;
mod il;
mod input;
mod metadata;
mod model;
#[cfg(feature = "csharp-decompiler")]
mod native;
mod refs;
mod resources;
mod scan;
mod signature;
mod strings;

pub use bundle::{Bundle, BundleEntry, select_module};
pub use decompile::{render, render_class};
pub use input::{Input, read_file};
pub use model::*;
pub use resources::{Resource, Resources};

pub use refs::{Reference, ReferenceResult, SemanticTarget, TargetResolver};

pub use scan::{IlScan, TokenSite};
