//! Engine conformance suite for wrkrs scripting engines.
//!
//! Defines the language specific script sources and the assertions
//! every engine must satisfy, plus the host fixtures the suite and the
//! engine unit tests share.

pub mod fixtures;
pub mod suite;

pub use suite::{MakeEngine, Scripts, run};
