//! The config engine, benchmark files instead of scripts.
//!
//! A TOML or JSON file shapes the load the way a static script would,
//! with request fields and stop conditions as data. Files parse
//! through one schema, see [`file`], and dispatch through the engine
//! registry on the `.toml` and `.json` extensions.

mod file;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use wrkrs_engine::{
    Capabilities, EngineError, ResolveApi, ScriptEngine, ScriptSpec, StatsView, Summary, ThreadApi,
    Value, format_request, host_header,
};

use self::file::{BenchFile, parse};

/// Engine driven by one benchmark file.
///
/// Every thread builds its own instance from the same spec, so stop
/// limits count per thread, the stop.lua behavior.
pub struct ConfigEngine {
    /// The formatted request bytes, rebuilt never, the file is data.
    request: Vec<u8>,
    /// The parsed benchmark file.
    file: BenchFile,
    /// Completed requests this thread saw.
    completed: u64,
    /// Response body bytes this thread saw.
    received: u64,
    /// The shared thread handle, delivered by init.
    thread: Option<Arc<dyn ThreadApi>>,
    /// Global storage, the cross environment transfer surface.
    globals: HashMap<String, Value>,
}

/// Registry factory that builds a [`ConfigEngine`].
pub fn factory(spec: &ScriptSpec) -> Result<Box<dyn ScriptEngine>, EngineError> {
    let engine = ConfigEngine::create(spec)?;
    Ok(Box::new(engine))
}

impl ScriptEngine for ConfigEngine {
    fn create(spec: &ScriptSpec) -> Result<Self, EngineError> {
        let path = spec.script.as_deref().ok_or_else(|| {
            EngineError::Load("config engine requires a benchmark file".to_owned())
        })?;
        let file = parse(path).map_err(|error| EngineError::Load(error.to_string()))?;
        let host = host_header(
            spec.parts.host.as_deref().unwrap_or_default(),
            spec.parts.port.as_deref(),
        );
        let request = format_request(
            file.request.method.as_deref().unwrap_or("GET"),
            file.request.path.as_deref().unwrap_or(&spec.parts.path),
            &overlay(&spec.headers, &file.request.headers),
            file.request.body.as_deref().map(str::as_bytes),
            Some(&host),
        );
        Ok(ConfigEngine {
            request,
            file,
            completed: 0,
            received: 0,
            thread: None,
            globals: HashMap::new(),
        })
    }

    fn resolve(
        &mut self,
        host: &str,
        service: &str,
        resolver: Arc<dyn ResolveApi>,
    ) -> Result<Vec<SocketAddr>, EngineError> {
        let addresses = resolver
            .lookup(host, service)
            .map_err(|error| EngineError::Resolve(error.to_string()))?;
        Ok(addresses
            .into_iter()
            .filter(|address| resolver.connect(address))
            .collect())
    }

    fn setup(&mut self, _thread: Arc<dyn ThreadApi>) -> Result<(), EngineError> {
        Ok(())
    }

    fn init(&mut self, thread: Arc<dyn ThreadApi>, _args: &[String]) -> Result<(), EngineError> {
        self.thread = Some(thread);
        Ok(())
    }

    fn delay(&mut self) -> u64 {
        0
    }

    fn request(&mut self) -> Result<Vec<u8>, EngineError> {
        Ok(self.request.clone())
    }

    fn response(
        &mut self,
        _status: u16,
        _headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(), EngineError> {
        self.completed += 1;
        self.received += body.len() as u64;
        if let Some(thread) = &self.thread
            && self.file.stop.reached(self.completed, self.received)
        {
            thread.stop();
        }
        Ok(())
    }

    fn done(
        &mut self,
        _summary: &Summary,
        _latency: Arc<dyn StatsView>,
        _requests: Arc<dyn StatsView>,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            is_static: true,
            wants_response: self.file.stop.is_set(),
            ..Capabilities::default()
        }
    }

    fn get_global(&self, name: &str) -> Result<Value, EngineError> {
        Ok(self.globals.get(name).cloned().unwrap_or(Value::Null))
    }

    fn set_global(&mut self, name: &str, value: &Value) -> Result<(), EngineError> {
        self.globals.insert(name.to_owned(), value.clone());
        Ok(())
    }
}

/// Layers file headers over the `-H` headers.
///
/// Same named entries replace in place, new names append, the
/// wrk.headers assignment rule scripts observe.
fn overlay(base: &[(String, String)], over: &[(String, String)]) -> Vec<(String, String)> {
    let mut headers = base.to_vec();
    for (name, value) in over {
        match headers.iter_mut().find(|(existing, _)| existing == name) {
            Some(entry) => entry.1 = value.clone(),
            None => headers.push((name.clone(), value.clone())),
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use super::ConfigEngine;
    use super::file::{FileError, parse};
    use wrkrs_engine::{ResolveApi, ScriptEngine, ScriptSpec, ThreadApi, UrlRef, Value};

    fn spec_with(script: Option<&std::path::Path>, headers: Vec<(String, String)>) -> ScriptSpec {
        ScriptSpec {
            url: "http://example.test:8080/".to_owned(),
            parts: UrlRef {
                scheme: Some("http".to_owned()),
                host: Some("example.test".to_owned()),
                port: Some("8080".to_owned()),
                path: "/".to_owned(),
            },
            script: script.map(std::path::PathBuf::from),
            headers,
        }
    }

    fn temp_file(name: &str, source: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, source).expect("write temporary file");
        path
    }

    #[test]
    fn builds_the_default_request_from_an_empty_file() {
        let script = temp_file("wrkrs-config-empty.toml", "");
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), vec![])).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET / HTTP/1.1\r\nHost: example.test:8080\r\n\r\n"
        );
        let capabilities = engine.capabilities();
        assert!(capabilities.is_static);
        assert!(!capabilities.wants_response);
    }

    #[test]
    fn builds_the_post_example_like_post_lua() {
        let script = temp_file(
            "wrkrs-config-post.toml",
            "[request]\nmethod = \"POST\"\nbody = \"foo=bar&baz=quux\"\n\
             [request.headers]\nContent-Type = \"application/x-www-form-urlencoded\"\n",
        );
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), vec![])).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"POST / HTTP/1.1\r\n\
              Content-Type: application/x-www-form-urlencoded\r\n\
              Host: example.test:8080\r\n\
              Content-Length: 16\r\n\
              \r\n\
              foo=bar&baz=quux"
        );
    }

    #[test]
    fn json_files_build_the_same_request() {
        let script = temp_file(
            "wrkrs-config-post.json",
            "{\"request\":{\"method\":\"POST\",\"body\":\"a=b\",\
             \"headers\":{\"Content-Type\":\"text/plain\"}}}",
        );
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), vec![])).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"POST / HTTP/1.1\r\n\
              Content-Type: text/plain\r\n\
              Host: example.test:8080\r\n\
              Content-Length: 3\r\n\
              \r\n\
              a=b"
        );
    }

    #[test]
    fn requires_a_benchmark_file() {
        let error = match ConfigEngine::create(&spec_with(None, vec![])) {
            Ok(_) => panic!("create without a file must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, wrkrs_engine::EngineError::Load(_)));
        assert_eq!(
            error.raw_message(),
            "config engine requires a benchmark file"
        );
    }

    #[test]
    fn parse_failures_surface_with_the_file_name() {
        let script = temp_file("wrkrs-config-bad.toml", "[request]\nmethod = ");
        let error = match ConfigEngine::create(&spec_with(Some(&script), vec![])) {
            Ok(_) => panic!("create with a broken file must fail"),
            Err(error) => error,
        };
        assert!(error.raw_message().contains("wrkrs-config-bad.toml"));
        let missing = std::env::temp_dir().join("wrkrs-config-absent.toml");
        let _ = std::fs::remove_file(&missing);
        let error = parse(&missing).unwrap_err();
        assert!(matches!(error, FileError::Open(_, _)));
    }

    #[test]
    fn file_headers_replace_dash_h_headers_in_place() {
        let script = temp_file(
            "wrkrs-config-overlay.toml",
            "[request.headers]\nAccept = \"text/html\"\nX-New = \"1\"\n",
        );
        let headers = vec![
            ("Accept".to_owned(), "*/*".to_owned()),
            ("User-Agent".to_owned(), "wrkrs".to_owned()),
        ];
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), headers)).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET / HTTP/1.1\r\n\
              Accept: text/html\r\n\
              User-Agent: wrkrs\r\n\
              X-New: 1\r\n\
              Host: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn an_empty_body_is_present_with_length_zero() {
        let script = temp_file(
            "wrkrs-config-empty-body.toml",
            "[request]\nmethod = \"POST\"\nbody = \"\"\n",
        );
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), vec![])).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"POST / HTTP/1.1\r\nHost: example.test:8080\r\nContent-Length: 0\r\n\r\n"
        );
    }

    struct FakeThread {
        stopped: std::sync::atomic::AtomicBool,
    }

    impl ThreadApi for FakeThread {
        fn addr(&self) -> Option<SocketAddr> {
            None
        }

        fn set_addr(&self, _addr: SocketAddr) {}

        fn stop(&self) {
            self.stopped
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }

        fn get_global(&self, _name: &str) -> Result<Value, wrkrs_engine::EngineError> {
            Ok(Value::Null)
        }

        fn set_global(&self, _name: &str, _value: &Value) -> Result<(), wrkrs_engine::EngineError> {
            Ok(())
        }
    }

    fn stopped_thread() -> (Arc<FakeThread>, Arc<dyn ThreadApi>) {
        let thread: Arc<FakeThread> = Arc::new(FakeThread {
            stopped: std::sync::atomic::AtomicBool::new(false),
        });
        let handle: Arc<dyn ThreadApi> = thread.clone();
        (thread, handle)
    }

    fn spec(script: &std::path::Path) -> ScriptSpec {
        spec_with(Some(script), vec![])
    }

    #[test]
    fn stop_requests_stops_the_thread_at_the_limit() {
        let script = temp_file("wrkrs-config-stop.toml", "[stop]\nrequests = 3\n");
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        assert!(engine.capabilities().wants_response);
        let (thread, handle) = stopped_thread();
        engine.init(handle, &[]).unwrap();
        for _ in 0..2 {
            engine.response(200, &[], b"ok").unwrap();
        }
        assert!(!thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
        engine.response(200, &[], b"ok").unwrap();
        assert!(thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn stop_bytes_stops_the_thread_at_the_limit() {
        let script = temp_file("wrkrs-config-stop-bytes.toml", "[stop]\nbytes = 6\n");
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        assert!(engine.capabilities().wants_response);
        let (thread, handle) = stopped_thread();
        engine.init(handle, &[]).unwrap();
        engine.response(200, &[], b"abc").unwrap();
        assert!(!thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
        engine.response(200, &[], b"abc").unwrap();
        assert!(thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn without_stop_conditions_responses_are_not_wanted() {
        let script = temp_file("wrkrs-config-nostop.toml", "[request]\npath = \"/x\"\n");
        let engine = ConfigEngine::create(&spec(&script)).unwrap();
        assert!(!engine.capabilities().wants_response);
    }

    #[test]
    fn globals_round_trip() {
        let script = temp_file("wrkrs-config-globals.toml", "");
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        assert_eq!(engine.get_global("id").unwrap(), Value::Null);
        engine.set_global("id", &Value::Int(7)).unwrap();
        assert_eq!(engine.get_global("id").unwrap(), Value::Int(7));
    }

    #[test]
    fn resolve_drops_unreachable_addresses() {
        struct FakeResolver {
            reachable: Vec<SocketAddr>,
        }

        impl ResolveApi for FakeResolver {
            fn lookup(
                &self,
                _host: &str,
                _service: &str,
            ) -> Result<Vec<SocketAddr>, std::io::Error> {
                Ok(vec![
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 9)), 8080),
                    self.reachable[0],
                ])
            }

            fn connect(&self, addr: &SocketAddr) -> bool {
                self.reachable.contains(addr)
            }
        }

        let script = temp_file("wrkrs-config-resolve.toml", "");
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        let reachable = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let resolver = FakeResolver {
            reachable: vec![reachable],
        };
        let resolved = engine
            .resolve("example.test", "8080", Arc::new(resolver))
            .unwrap();
        assert_eq!(resolved, vec![reachable]);
    }
}
