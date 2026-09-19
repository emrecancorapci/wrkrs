//! Scripting engine contract for wrkrs.
//!
//! wrkrs drives a benchmark through a scripting engine. This crate defines
//! the types and the [`ScriptEngine`] trait an implementation provides:
//!
//! - [`ScriptSpec`] and [`UrlRef`] describe one scripting environment
//! - [`ScriptEngine`] carries a run through the wrk phases
//! - [`Value`] models the values transferable across environments
//! - [`ThreadApi`], [`ResolveApi`], and [`StatsView`] expose host services
//! - [`Capabilities`] tells the host what the loaded script needs
//! - [`format_request`] and [`host_header`] share wrk request formatting
//!
//! The contract preserves wrk's observable script behavior, including its
//! quirks. Anything documented as a quirk is an acceptance criterion.

#![warn(missing_docs)]

mod api;
mod capabilities;
mod engine;
mod error;
mod spec;
mod value;

pub use api::{ErrorCounts, ResolveApi, StatsView, Summary, ThreadApi};
pub use capabilities::Capabilities;
pub use engine::ScriptEngine;
pub use error::EngineError;
pub use spec::{ScriptSpec, UrlRef};
pub use value::Value;
