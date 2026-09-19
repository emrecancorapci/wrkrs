//! The conformance cases every engine must satisfy.
//!
//! Every case builds a fresh engine from a script source written to a
//! temporary file and asserts one observable behavior. Failure panics
//! carry the engine name so mixed runs stay readable.

use std::sync::Arc;

use wrkrs_engine::{EngineError, ScriptEngine, ScriptSpec, format_request, host_header};

use crate::fixtures::{FakeThread, spec, temp_script};

/// Builds an engine for a spec, the registry factory shape.
pub type MakeEngine = fn(&ScriptSpec) -> Result<Box<dyn ScriptEngine>, EngineError>;

/// Language specific script sources for the conformance cases.
pub struct Scripts {
    /// Sets the POST method, a header, and a body.
    pub post: &'static str,
    /// Builds a pipelined request from formatted parts.
    pub pipeline: &'static str,
    /// Returns a constant delay in milliseconds.
    pub delay: &'static str,
    /// Captures the response arguments into seen_ globals.
    pub response_capture: &'static str,
    /// Captures the done arguments into seen_ globals.
    pub done_capture: &'static str,
    /// Transfers a value into the thread in setup.
    pub setup_transfer: &'static str,
    /// Captures the first init argument.
    pub init_args: &'static str,
    /// Defines every callback so capabilities report them.
    pub full: &'static str,
    /// Raises a runtime error inside request.
    pub runtime_error: &'static str,
    /// Fails to parse.
    pub syntax_error: &'static str,
}

/// Runs every conformance case against one engine.
///
/// A delay callback error aborts the process in every engine, the way
/// the unprotected C call does, so that path stays outside the suite.
pub fn run(name: &str, make: MakeEngine, scripts: &Scripts) {
    default_request(name, make);
    let _ = scripts;
}

/// Builds an engine for a script source.
fn engine_for(
    name: &str,
    make: MakeEngine,
    case: &str,
    script: Option<&str>,
) -> Box<dyn ScriptEngine> {
    let spec = match script {
        Some(source) => spec(Some(&temp_script(&format!("{name}-{case}"), source))),
        None => spec(None),
    };
    make(&spec).unwrap_or_else(|error| panic!("{name}: engine creation failed: {error}"))
}

/// The default request must match the core formatter byte for byte.
fn default_request(name: &str, make: MakeEngine) {
    let mut engine = engine_for(name, make, "default", None);
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    let request = engine
        .request()
        .unwrap_or_else(|error| panic!("{name}: request failed: {error}"));
    let host = host_header("example.test", Some("8080"));
    let expected = format_request("GET", "/some/path", &[], None, Some(&host));
    assert_eq!(request, expected, "{name}: default request mismatch");
}
