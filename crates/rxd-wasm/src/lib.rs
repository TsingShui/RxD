//! Static WebAssembly module structure, strings and explicit reference scanning.
//!
//! Reads a binary core wasm module as one fixed buffer; never instantiates or
//! executes it. Decode and validation use a pinned `wasmparser`.
mod decompile;
mod model;
mod module;
mod refs;
mod strings;
mod view;

pub use model::*;
pub use module::{DecodedBody, DecodedOp, LocalDecl, Module, OpSummary};
pub use refs::refs;
pub use strings::strings;
pub use view::instruction_view;

use anyhow::Result;

/// Resolve a single function for `get`: attempt structured decompilation by
/// default and fall back to the conservative instruction view on any uncovered
/// or non-structured situation. The fallback carries the degrade reason.
pub fn get_view(module: &Module, position: usize, raw: bool) -> Result<FunctionView> {
    if !raw {
        match decompile::view(module, position)? {
            Ok(view) => return Ok(view),
            Err(reason) => {
                let mut view = instruction_view(module, position)?;
                view.reason = Some(reason);
                return Ok(view);
            }
        }
    }
    instruction_view(module, position)
}
