//! Static WASM structure, instructions and references using pinned wasmparser.
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

/// Prefer structured decompilation; unsupported cases return instructions with a reason.
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
