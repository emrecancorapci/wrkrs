//! Orchestrates engines for a run: spec building, resolution, thread
//! setup, and the thread zero probes.

use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use wrkrs_engine::ScriptSpec;
use wrkrs_engine::{EngineError, ScriptEngine, ThreadApi, Value};

use crate::cli::Config;

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
    use super::{HostThread, build_spec, split_headers};
    use crate::cli::Config;
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
}
