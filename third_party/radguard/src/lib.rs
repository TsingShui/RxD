//! RadGuard reconstructs Java-like pseudocode from Android DEX bytecode.
//!
//! The library surface is intentionally narrow, because it is the contract that
//! other projects embed:
//!
//! ```no_run
//! # let dex_bytes: Vec<u8> = Vec::new();
//! let decompiler = radguard::Decompiler::parse(&dex_bytes)?;
//! match decompiler.decompile_class("Lcom/example/MainActivity;")? {
//!     Some(output) => println!("{}", output.source),
//!     None => eprintln!("class is not defined in this DEX"),
//! }
//! # Ok::<(), radguard::Error>(())
//! ```
//!
//! * [`Decompiler::parse`] borrows the DEX bytes; [`Decompiler::from_arc`] shares
//!   them with other threads instead of copying.
//! * [`Decompiler::decompile_class`] answers `Ok(None)` when the DEX does not
//!   define the requested descriptor, which callers use to distinguish
//!   "not found" from "failed to decompile".
//! * [`SourceOutput`] owns its rendered source and the member references inside
//!   it; nothing borrows from the decompiler.
//!
//! Everything below that facade - the DEX tables, the IR, the control-flow and
//! dataflow passes, and the Java writer - is implementation detail.
//!
//! This copy is rxd's vendored fork: the command-line shell, the APK pipeline and
//! the C decompression backend are not carried. See `FORK.md`.

// The public surface is deliberately narrow, so items that only the DEX path or
// the crate's own tests reach are unreachable from outside and look dead.
#![allow(dead_code)]

pub(crate) mod analysis;
pub mod backend;
pub(crate) mod control_flow;
pub(crate) mod dataflow;
pub mod decompiler;
pub mod dex;
pub mod error;
pub(crate) mod graph;
pub(crate) mod ir;
pub(crate) mod render;
pub(crate) mod types;
pub(crate) mod writer;

pub use decompiler::{Decompiler, decompile_dex_bytes};
pub use dex::DexFile;
pub use error::{Error, Result};
pub use writer::{MemberKind, MemberReference, SourceOutput};
