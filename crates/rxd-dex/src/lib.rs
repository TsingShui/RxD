//! Native APK/DEX analysis migrated from rasc; never executes analyzed code.
pub mod apk;
mod archive;
mod bytes;
mod dex;
pub use dex::hooks;
pub mod diag;
mod emitter;
pub mod manifest;
pub mod query;
