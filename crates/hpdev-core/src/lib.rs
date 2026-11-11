//! Adapters around the upstream PPL compiler, VM, host and graphics APIs.
pub mod debug;
pub mod execution;
pub mod host;
pub mod model;
pub mod protocol;
pub mod testing;

pub use model::*;
pub use ppl_engine;
pub use ppl_graphics;
