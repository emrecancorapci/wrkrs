//! The conformance cases every engine must satisfy.
//!
//! Every case builds a fresh engine from a script source written to a
//! temporary file and asserts one observable behavior. Failure panics
//! carry the engine name so mixed runs stay readable.

use std::sync::Arc;

use wrkrs_engine::{
    EngineError, ScriptEngine, ScriptSpec, ThreadApi, Value, format_request, host_header,
};

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
    /// Transfers a function value into the thread, which must fail.
    pub transfer_reject: &'static str,
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
    done(name, make, scripts.done_capture);
    setup_and_init(name, make, scripts);
    capabilities(name, make, scripts.full);
    errors(name, make, scripts);
}

/// Script errors follow the wrk rules: load failures keep the default
/// behavior, runtime failures surface through the callback result.
fn errors(name: &str, make: MakeEngine, scripts: &Scripts) {
    // a broken script still yields a working default environment
    let mut engine = engine_for(name, make, "syntax", Some(scripts.syntax_error));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    assert!(
        engine.capabilities().is_static,
        "{name}: broken script must fall back to defaults"
    );
    let host = host_header("example.test", Some("8080"));
    let expected = format_request("GET", "/some/path", &[], None, Some(&host));
    assert_eq!(
        engine.request().unwrap_or_else(|e| panic!("{name}: {e}")),
        expected,
        "{name}: broken script must still produce the default request"
    );

    // a runtime error inside request surfaces to the host
    let mut engine = engine_for(name, make, "runtime", Some(scripts.runtime_error));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    let error = engine
        .request()
        .err()
        .unwrap_or_else(|| panic!("{name}: request error must surface"));
    assert!(
        error.raw_message().contains("boom"),
        "{name}: unexpected request error: {error}"
    );
}

/// Capabilities must reflect what the loaded script defines.
fn capabilities(name: &str, make: MakeEngine, script: &str) {
    let mut engine = engine_for(name, make, "caps-default", None);
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    let reported = engine.capabilities();
    assert!(reported.is_static, "{name}: default must be static");
    assert!(
        !reported.wants_response,
        "{name}: default wants no response"
    );
    assert!(!reported.has_delay, "{name}: default has no delay");
    assert!(!reported.has_done, "{name}: default has no done");

    let mut engine = engine_for(name, make, "caps-full", Some(script));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    let reported = engine.capabilities();
    assert!(!reported.is_static, "{name}: request global means dynamic");
    assert!(
        reported.wants_response,
        "{name}: response global must report"
    );
    assert!(reported.has_delay, "{name}: delay global must report");
    assert!(reported.has_done, "{name}: done global must report");
}

/// Setup and init wire the thread: values transfer in, the first
/// argument lands at index zero, and unsupported values are rejected
/// with the wrk message.
fn setup_and_init(name: &str, make: MakeEngine, scripts: &Scripts) {
    use crate::fixtures::{FakeResolver, FakeThread};

    // setup transfers a value into the thread environment
    let mut engine = engine_for(name, make, "setup", Some(scripts.setup_transfer));
    engine
        .resolve(
            "example.test",
            "8080",
            std::sync::Arc::new(FakeResolver::default()),
        )
        .unwrap_or_else(|error| panic!("{name}: resolve failed: {error}"));
    let thread = Arc::new(FakeThread::default());
    engine
        .setup(thread.clone())
        .unwrap_or_else(|error| panic!("{name}: setup failed: {error}"));
    assert_eq!(
        thread
            .get_global("id")
            .unwrap_or_else(|e| panic!("{name}: {e}")),
        Value::Int(7),
        "{name}: setup transfer mismatch"
    );
    assert_eq!(
        thread.addr(),
        Some(crate::fixtures::address(1)),
        "{name}: setup must assign the first address"
    );

    // init receives the extra arguments from index zero
    let mut engine = engine_for(name, make, "args", Some(scripts.init_args));
    engine
        .init(
            Arc::new(FakeThread::default()),
            &["hello".to_owned(), "world".to_owned()],
        )
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    assert_eq!(
        engine
            .get_global("seen_first")
            .unwrap_or_else(|error| panic!("{name}: args read failed: {error}")),
        Value::Str("hello".to_owned()),
        "{name}: first argument must land at index zero"
    );

    // a function value cannot transfer into the thread
    let mut engine = engine_for(name, make, "reject", Some(scripts.transfer_reject));
    engine
        .resolve(
            "example.test",
            "8080",
            std::sync::Arc::new(FakeResolver::default()),
        )
        .unwrap_or_else(|error| panic!("{name}: resolve failed: {error}"));
    let error = engine
        .setup(Arc::new(FakeThread::default()))
        .err()
        .unwrap_or_else(|| panic!("{name}: function transfer must fail"));
    assert!(
        error
            .to_string()
            .contains("cannot transfer 'function' to thread"),
        "{name}: unexpected transfer error: {error}"
    );
}

/// The done callback reads the summary fields and both stats objects.
fn done(name: &str, make: MakeEngine, script: &str) {
    use crate::fixtures::FakeStats;
    use wrkrs_engine::{ErrorCounts, Summary};

    let mut engine = engine_for(name, make, "done", Some(script));
    engine
        .init(Arc::new(FakeThread::default()), &[])
        .unwrap_or_else(|error| panic!("{name}: init failed: {error}"));
    engine
        .done(
            &Summary {
                duration: 5_000_000,
                requests: 120,
                bytes: 4096,
                errors: ErrorCounts {
                    connect: 2,
                    ..ErrorCounts::default()
                },
            },
            Arc::new(FakeStats),
            Arc::new(FakeStats),
        )
        .unwrap_or_else(|error| panic!("{name}: done failed: {error}"));
    let expected = [
        ("seen_duration", Value::Int(5_000_000)),
        ("seen_connect", Value::Int(2)),
        ("seen_percentile", Value::Int(100)),
        ("seen_length", Value::Int(7)),
    ];
    for (key, value) in expected {
        assert_eq!(
            engine
                .get_global(key)
                .unwrap_or_else(|error| panic!("{name}: {key} read failed: {error}")),
            value,
            "{name}: done {key} mismatch"
        );
    }
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
