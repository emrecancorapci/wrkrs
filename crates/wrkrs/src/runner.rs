//! Orchestrates engines for a run: spec building, resolution, thread
//! setup, and the thread zero probes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use wrkrs_engine::ScriptSpec;
use wrkrs_engine::{Capabilities, EngineError, ErrorCounts, ScriptEngine, ThreadApi, Value};

use crate::cli::Config;
use crate::engines::EngineEntry;
use crate::eventloop::{self, StopFlag};
use crate::parser::verify_request;
use crate::report::RunReport;
use crate::resolve::SystemResolver;
use crate::signals;
use crate::stats::Histogram;
use crate::tls::TlsSetup;

/// The per thread rate histogram ceiling, MAX_THREAD_RATE_S in wrk.h.
const MAX_THREAD_RATE: u64 = 10_000_000;

/// Runs a prepared benchmark: spawns the workers, waits out the
/// duration, aggregates the counters, and applies the coordinated
/// omission correction. The main engine comes back for the done phase
/// after the report prints.
pub fn execute(config: &Config, prepared: Prepared) -> (RunReport, Box<dyn ScriptEngine>) {
    signals::install();
    let Prepared {
        pipeline,
        capabilities,
        threads,
        addresses,
        tls,
        main,
    } = prepared;

    let latency = Arc::new(Histogram::new(config.timeout_ms.saturating_mul(1000)));
    let rate = Arc::new(Histogram::new(MAX_THREAD_RATE));
    let stop = Arc::new(StopFlag::new());
    let per_thread = config.connections / config.threads;

    let mut workers = Vec::new();
    for handle in &threads {
        let engine = handle.take().expect("the engine parked in prepare");
        let address = handle
            .addr()
            .or_else(|| addresses.first().copied())
            .expect("prepare resolved an address");
        let worker = Arc::clone(handle);
        let latency = Arc::clone(&latency);
        let rate = Arc::clone(&rate);
        let stop = Arc::clone(&stop);
        let dynamic = !capabilities.is_static;
        let has_delay = capabilities.has_delay;
        let wants_response = capabilities.wants_response;
        let tls = tls.clone();
        workers.push(thread::spawn(move || {
            eventloop::run(
                address,
                per_thread as usize,
                engine,
                worker,
                latency,
                rate,
                stop,
                pipeline,
                dynamic,
                has_delay,
                wants_response,
                tls,
            )
        }));
    }

    let start = Instant::now();
    let deadline = start + Duration::from_secs(config.duration_s);
    while Instant::now() < deadline {
        if signals::interrupted() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    stop.stop();
    let duration_us = start.elapsed().as_micros() as u64;

    let mut complete = 0;
    let mut bytes = 0;
    let mut errors = ErrorCounts::default();
    for worker in workers {
        let counters = worker.join().expect("worker finishes");
        complete += counters.complete;
        bytes += counters.bytes;
        errors.connect += counters.errors.connect;
        errors.read += counters.errors.read;
        errors.write += counters.errors.write;
        errors.status += counters.errors.status;
        errors.timeout += counters.errors.timeout;
    }

    // The coordinated omission correction over the average interval.
    if config.connections > 0 && complete / config.connections > 0 {
        let interval = duration_us / (complete / config.connections);
        latency.correct(interval as i64);
    }

    (
        RunReport {
            duration_us,
            complete,
            bytes,
            errors,
            latency,
            rate,
        },
        main,
    )
}

/// Everything the run loop needs after preparation.
pub struct Prepared {
    /// Requests per pipeline from the thread zero probe.
    pub pipeline: u64,
    /// What the loaded script demands from the run loop.
    pub capabilities: Capabilities,
    /// One handle per thread, the engines parked inside.
    pub threads: Vec<Arc<HostThread>>,
    /// The reachable addresses in resolver order.
    pub addresses: Vec<SocketAddr>,
    /// The TLS setup for an https target.
    pub tls: Option<Arc<TlsSetup>>,
    /// The main engine for the done phase.
    pub main: Box<dyn ScriptEngine>,
}

/// A preparation failure, printed to stderr with exit one.
#[derive(Debug)]
pub struct PrepareError(String);

impl PrepareError {
    /// The message to print.
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// Prepares a run: resolution, thread setup, and the thread zero
/// probes.
///
/// Mirrors the wrk main flow up to the point where threads spawn.
/// Resolution failures and unreachable targets keep the wrk messages,
/// script callback errors surface where C aborts through its
/// unprotected calls.
pub fn prepare(config: &Config, entry: &EngineEntry) -> Result<Prepared, PrepareError> {
    let spec = build_spec(config);
    let resolver = Arc::new(SystemResolver::new());

    let mut main =
        (entry.factory)(&spec).map_err(|error| PrepareError(error.raw_message().to_owned()))?;

    // wrk resolves the host against the port or the scheme name.
    let host = config.parts.host.clone().unwrap_or_default();
    // C matches any schema starting with https, a five character
    // strncmp.
    let tls = if config
        .parts
        .scheme
        .as_deref()
        .is_some_and(|scheme| scheme.starts_with("https"))
    {
        Some(Arc::new(TlsSetup::new(&host)))
    } else {
        None
    };
    let service = config
        .parts
        .port
        .clone()
        .or_else(|| config.parts.scheme.clone())
        .unwrap_or_default();
    let addresses = main
        .resolve(&host, &service, resolver.clone())
        .map_err(|error| PrepareError(error.raw_message().to_owned()))?;
    if addresses.is_empty() {
        return Err(PrepareError(format!(
            "unable to connect to {host}:{service} {}",
            resolver.last_connect_error()
        )));
    }

    let mut threads = Vec::new();
    let mut pipeline = 1;
    let mut capabilities = Capabilities::default();
    for index in 0..config.threads {
        let engine =
            (entry.factory)(&spec).map_err(|error| PrepareError(error.raw_message().to_owned()))?;
        let handle = Arc::new(HostThread::new());
        handle.park(engine);

        main.setup(handle.clone())
            .map_err(|error| PrepareError(error.raw_message().to_owned()))?;

        let mut engine = handle.take().expect("engine parked above");
        engine
            .init(handle.clone(), &config.init_args)
            .map_err(|error| PrepareError(error.raw_message().to_owned()))?;

        if index == 0 {
            capabilities = engine.capabilities();
            let request = engine
                .request()
                .map_err(|error| PrepareError(error.raw_message().to_owned()))?;
            pipeline = verify_request(&request).map_err(PrepareError)?;
        }

        handle.park(engine);
        threads.push(handle);
    }

    Ok(Prepared {
        pipeline,
        capabilities,
        threads,
        addresses,
        tls,
        main,
    })
}

/// The host side of one benchmark thread.
///
/// The handle carries the connection address and stop flag and parks
/// the thread engine so setup and done reach its globals, the same
/// lifetimes the thread userdata has in C. The worker takes the
/// engine out for the run and parks it back afterwards.
pub struct HostThread {
    addr: Mutex<Option<SocketAddr>>,
    stopped: AtomicBool,
    engine: Mutex<Option<Box<dyn ScriptEngine>>>,
}

impl HostThread {
    /// Creates a handle with no engine parked.
    pub fn new() -> HostThread {
        HostThread {
            addr: Mutex::new(None),
            stopped: AtomicBool::new(false),
            engine: Mutex::new(None),
        }
    }

    /// Parks the thread engine so the handle reaches its globals.
    pub fn park(&self, engine: Box<dyn ScriptEngine>) {
        *self
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(engine);
    }

    /// Takes the engine out for its worker thread.
    pub fn take(&self) -> Option<Box<dyn ScriptEngine>> {
        self.engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// Whether stop was requested.
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
}

impl Default for HostThread {
    fn default() -> Self {
        HostThread::new()
    }
}

impl ThreadApi for HostThread {
    fn addr(&self) -> Option<SocketAddr> {
        *self
            .addr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_addr(&self, address: SocketAddr) {
        *self
            .addr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(address);
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    fn get_global(&self, name: &str) -> Result<Value, EngineError> {
        match self
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            Some(engine) => engine.get_global(name),
            None => Err(EngineError::Runtime(
                "thread engine is unavailable".to_owned(),
            )),
        }
    }

    fn set_global(&self, name: &str, value: &Value) -> Result<(), EngineError> {
        match self
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_mut()
        {
            Some(engine) => engine.set_global(name, value),
            None => Err(EngineError::Runtime(
                "thread engine is unavailable".to_owned(),
            )),
        }
    }
}

/// Builds the engine spec from the run configuration.
pub fn build_spec(config: &Config) -> ScriptSpec {
    ScriptSpec {
        url: config.url.clone(),
        parts: config.parts.clone(),
        script: config.script.clone(),
        headers: split_headers(&config.headers),
    }
}

/// Splits raw header arguments at the first colon followed by a space.
///
/// Headers without that separator drop silently, matching the
/// script.c loop over `-H` values.
fn split_headers(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|header| {
            let colon = header.find(':')?;
            let split = header.as_bytes().get(colon + 1) == Some(&b' ');
            split.then(|| {
                let name = header[..colon].to_owned();
                let value = header[colon + 2..].to_owned();
                (name, value)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::HostThread;
    use super::{build_spec, split_headers};
    use crate::cli::Config;
    use crate::engines::EngineEntry;
    use crate::parser::parse_url;

    fn config(headers: &[&str]) -> Config {
        Config {
            threads: 2,
            connections: 10,
            duration_s: 10,
            timeout_ms: 2000,
            latency: false,
            script: None,
            headers: headers.iter().map(|header| (*header).to_owned()).collect(),
            engine: None,
            output: Default::default(),
            output_file: None,
            url: "http://host/".to_owned(),
            parts: parse_url("http://host/").expect("valid url"),
            init_args: vec!["http://host/".to_owned()],
        }
    }

    #[test]
    fn splits_headers_at_the_first_separator() {
        let split = split_headers(&[
            "Accept: application/json".to_owned(),
            "X-Weird: A: B".to_owned(),
        ]);
        assert_eq!(
            split,
            vec![
                ("Accept".to_owned(), "application/json".to_owned()),
                ("X-Weird".to_owned(), "A: B".to_owned()),
            ]
        );
    }

    #[test]
    fn headers_without_the_separator_drop() {
        assert!(split_headers(&["Broken".to_owned()]).is_empty());
        assert!(split_headers(&["A:B".to_owned()]).is_empty());
        // An empty name still passes, the C loop keeps it.
        assert_eq!(
            split_headers(&[": value".to_owned()]),
            vec![("".to_owned(), "value".to_owned())]
        );
    }

    #[test]
    fn the_spec_carries_the_url_parts_and_script() {
        let config = config(&[]);
        let spec = build_spec(&config);
        assert_eq!(spec.url, "http://host/");
        assert_eq!(spec.parts.path, "/");
        assert!(spec.script.is_none());
        assert!(spec.headers.is_empty());
    }

    #[test]
    fn the_handle_tracks_address_and_stop() {
        use wrkrs_engine::ThreadApi;

        let handle = HostThread::new();
        assert_eq!(handle.addr(), None);
        handle.set_addr("127.0.0.1:80".parse().unwrap());
        assert_eq!(handle.addr(), Some("127.0.0.1:80".parse().unwrap()));
        assert!(!handle.stopped());
        handle.stop();
        assert!(handle.stopped());
    }

    #[cfg(any(feature = "engine-luajit", feature = "engine-lua54"))]
    #[test]
    fn parked_engines_serve_globals_through_the_handle() {
        use std::sync::Arc;
        use wrkrs_engine::ThreadApi;

        let factory = crate::engines::find_by_extension("probe.lua")
            .expect("the lua engine is compiled in")
            .factory;
        let spec = build_spec(&config(&[]));
        let engine = factory(&spec).expect("engine builds");
        let handle = Arc::new(HostThread::new());
        handle.park(engine);
        handle
            .set_global("id", &wrkrs_engine::Value::Int(7))
            .expect("set works while parked");
        assert_eq!(
            handle.get_global("id").expect("id is set"),
            wrkrs_engine::Value::Int(7)
        );
    }

    #[test]
    fn taking_the_engine_closes_the_globals() {
        use wrkrs_engine::ThreadApi;

        let handle = HostThread::new();
        let error = handle.get_global("anything").expect_err("no engine parked");
        assert_eq!(error.raw_message(), "thread engine is unavailable");
        assert!(handle.take().is_none());
    }

    #[cfg(any(feature = "engine-luajit", feature = "engine-lua54"))]
    fn prepared_config(name: &str, url: &str, script: &str) -> (Config, &'static EngineEntry) {
        let path = std::env::temp_dir().join(format!("wrkrs-runner-{name}.lua"));
        std::fs::write(&path, script).expect("write temporary script");
        let entry = crate::engines::find_by_extension("bench.lua").expect("lua engine");
        let mut config = config(&[]);
        config.url = url.to_owned();
        config.parts = parse_url(url).expect("valid url");
        config.script = Some(path);
        config.init_args = vec![url.to_owned()];
        (config, entry)
    }

    #[cfg(any(feature = "engine-luajit", feature = "engine-lua54"))]
    #[test]
    fn prepares_a_full_run() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local address");
        let url = format!("http://127.0.0.1:{}/", address.port());
        let (config, entry) = prepared_config(
            "full",
            &url,
            "function request() return \"GET / HTTP/1.1\\r\\nHost: h\\r\\n\\r\\n\" end\n\
             function response(status, headers, body) end\n\
             function delay() return 0 end\n\
             function done(summary, latency, requests) end\n",
        );

        let prepared = super::prepare(&config, entry).expect("preparation succeeds");
        assert_eq!(prepared.pipeline, 1);
        assert!(!prepared.capabilities.is_static);
        assert!(prepared.capabilities.wants_response);
        assert!(prepared.capabilities.has_delay);
        assert!(prepared.capabilities.has_done);
        assert_eq!(prepared.threads.len(), 2);
        assert_eq!(prepared.addresses, vec![address]);
    }

    #[cfg(any(feature = "engine-luajit", feature = "engine-lua54"))]
    #[test]
    fn pipelined_scripts_report_their_depth() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local address");
        let url = format!("http://127.0.0.1:{}/", address.port());
        let (config, entry) = prepared_config(
            "pipeline",
            &url,
            "function init(args)\n\
             req = wrk.format(nil, \"/?foo\") .. wrk.format(nil, \"/?bar\") .. wrk.format(nil, \"/?baz\")\n\
             end\n\
             function request() return req end\n",
        );

        let prepared = super::prepare(&config, entry).expect("preparation succeeds");
        assert_eq!(prepared.pipeline, 3);
        assert!(!prepared.capabilities.is_static);
    }

    #[test]
    fn unreachable_targets_keep_the_wrk_message() {
        use std::net::TcpListener;

        // One listening and one refused address so the resolver order
        // survives, then probe against a port with nothing behind it.
        let _listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let (config, entry) = prepared_config("refused", "http://127.0.0.1:1/", "");
        let _ = entry;
        let error = super::prepare(&config, entry)
            .err()
            .expect("nothing listens on port one");
        assert_eq!(
            error.message(),
            "unable to connect to 127.0.0.1:1 Connection refused"
        );
    }
}
