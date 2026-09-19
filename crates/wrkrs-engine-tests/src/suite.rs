//! The conformance cases every engine must satisfy.
//!
//! Every case builds a fresh engine from a script source written to a
//! temporary file and asserts one observable behavior. Failure panics
//! carry the engine name so mixed runs stay readable.

use std::sync::Arc;

use wrkrs_engine::{EngineError, ScriptEngine, ScriptSpec, Value, format_request, host_header};

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
    post(name, make, scripts.post);
    pipeline(name, make, scripts.pipeline);
    delay(name, make, scripts.delay);
    response(name, make, scripts.response_capture);
}

/// The response callback receives the status, headers, and body
/// faithfully, with duplicate header names keeping the last value.
fn response(name: &str, make: MakeEngine, script: &str) {
    let mut engine = engine_for(name, make, "response", Some(script));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    engine
        .response(
            201,
            &[
                ("Content-Type".to_owned(), "text/plain".to_owned()),
                ("X-Dup".to_owned(), "first".to_owned()),
                ("X-Dup".to_owned(), "second".to_owned()),
            ],
            b"payload",
        )
        .unwrap_or_else(|error| panic!("{name}: response failed: {error}"));
    assert_eq!(
        engine
            .get_global("seen_status")
            .unwrap_or_else(|error| panic!("{name}: status read failed: {error}")),
        Value::Int(201),
        "{name}: response status mismatch"
    );
    assert_eq!(
        engine
            .get_global("seen_type")
            .unwrap_or_else(|error| panic!("{name}: header read failed: {error}")),
        Value::Str("text/plain".to_owned()),
        "{name}: response header mismatch"
    );
    assert_eq!(
        engine
            .get_global("seen_dup")
            .unwrap_or_else(|error| panic!("{name}: duplicate read failed: {error}")),
        Value::Str("second".to_owned()),
        "{name}: duplicate header must keep the last value"
    );
    assert_eq!(
        engine
            .get_global("seen_body")
            .unwrap_or_else(|error| panic!("{name}: body read failed: {error}")),
        Value::Str("payload".to_owned()),
        "{name}: response body mismatch"
    );
}

/// The delay callback value reaches the host as milliseconds.
fn delay(name: &str, make: MakeEngine, script: &str) {
    let mut engine = engine_for(name, make, "delay", Some(script));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    assert_eq!(engine.delay(), 125, "{name}: delay value mismatch");
}

/// A pipelined request is the exact concatenation of the formatted
/// parts.
fn pipeline(name: &str, make: MakeEngine, script: &str) {
    let mut engine = engine_for(name, make, "pipeline", Some(script));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    let request = engine
        .request()
        .unwrap_or_else(|error| panic!("{name}: request failed: {error}"));
    let host = host_header("example.test", Some("8080"));
    let expected = [
        format_request("GET", "/?foo", &[], None, Some(&host)),
        format_request("GET", "/?bar", &[], None, Some(&host)),
    ]
    .concat();
    assert_eq!(request, expected, "{name}: pipelined request mismatch");
}

/// A script that changes the method, headers, and body produces the
/// matching request bytes.
///
/// Header order follows the scripting VM, so the case compares the
/// header set rather than the sequence.
fn post(name: &str, make: MakeEngine, script: &str) {
    let mut engine = engine_for(name, make, "post", Some(script));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    let request = engine
        .request()
        .unwrap_or_else(|error| panic!("{name}: request failed: {error}"));
    let host = host_header("example.test", Some("8080"));
    let expected = format_request(
        "POST",
        "/some/path",
        &[("Content-Type".to_owned(), "text/plain".to_owned())],
        Some(b"hello".as_slice()),
        Some(&host),
    );
    assert_eq!(
        request_parts(&request),
        request_parts(&expected),
        "{name}: post request mismatch"
    );
}

/// Splits a request into its request line, sorted header lines, and
/// body so header order does not affect comparison.
fn request_parts(request: &[u8]) -> (String, Vec<String>, Vec<u8>) {
    let text = String::from_utf8_lossy(request);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("request has no header terminator: {text:?}"));
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut headers: Vec<String> = lines.map(str::to_owned).collect();
    headers.sort();
    (request_line, headers, body.as_bytes().to_vec())
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
