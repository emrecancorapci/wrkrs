//! The config engine, benchmark files instead of scripts.
//!
//! A TOML or JSON file shapes the load the way a static script would,
//! with request fields and stop conditions as data, light templating
//! and one capture rule for the dynamic shapes. Files parse through
//! one schema, see [`file`], template tags parse through [`template`],
//! and both dispatch through the engine registry on the `.toml` and
//! `.json` extensions.

mod file;
mod template;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use wrkrs_engine::{
    Capabilities, EngineError, ResolveApi, ScriptEngine, ScriptSpec, StatsView, Summary, ThreadApi,
    Value, format_request, host_header,
};

use self::file::{BenchFile, parse};
use self::template::{Rng, Template};

/// Engine driven by one benchmark file.
///
/// Every thread builds its own instance from the same spec, so stop
/// limits, the `{{n}}` counter, and a capture rule count per thread,
/// the stop.lua, counter.lua, and auth.lua behavior.
pub struct ConfigEngine {
    /// The formatted request bytes of a literal file, rebuilt never,
    /// the file is data.
    request: Vec<u8>,
    /// The render pieces of a dynamic file, one render per burst.
    dynamic: Option<DynamicRequest>,
    /// The `-H` values under the file headers, the overlay base.
    base_headers: Vec<(String, String)>,
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

/// The render pieces of a dynamic request.
///
/// One render builds one request: every tag in it draws from one
/// counter value, so the path, body, and headers of a request agree.
struct DynamicRequest {
    /// The HTTP method, methods are not template fields.
    method: String,
    /// The path template.
    path: Template,
    /// File header templates, rendered then layered over `-H`.
    headers: Vec<(String, Template)>,
    /// The body template, absent means none.
    body: Option<Template>,
    /// The default Host value, replaced by a file Host entry.
    host: String,
    /// The value rendered into the next `{{n}}`, starting at one.
    counter: u64,
    /// The random source behind `{{rand}}`.
    rng: Rng,
    /// The capture rule state, absent means no capture.
    capture: Option<DynamicCapture>,
}

/// The runtime state of one capture rule.
///
/// The rule lives in the dynamic request because it is the third
/// dynamic shape: the request changes when the capture lands.
struct DynamicCapture {
    /// The response header to copy, matched case insensitively.
    header: String,
    /// The request header that carries the captured value.
    to: String,
    /// The path of the requests after the capture, absent keeps the
    /// request path.
    path: Option<Template>,
    /// The captured value, absent until the first response carries
    /// the header.
    value: Option<String>,
}

impl DynamicCapture {
    /// Offers one response to the rule.
    ///
    /// The first response carrying the header wins and later
    /// responses never refresh the value, the `if not token` guard
    /// of auth.lua.
    fn offer(&mut self, headers: &[(String, String)]) {
        if self.value.is_some() {
            return;
        }
        self.value = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&self.header))
            .map(|(_, value)| value.clone());
    }
}

impl DynamicRequest {
    /// Renders the next request over the `-H` header base.
    fn render(&mut self, base: &[(String, String)]) -> Vec<u8> {
        let counter = self.counter;
        self.counter += 1;
        let captured = self
            .capture
            .as_ref()
            .and_then(|capture| capture.value.clone());
        let path = match (&self.capture, &captured) {
            (Some(capture), Some(_)) => capture
                .path
                .as_ref()
                .unwrap_or(&self.path)
                .render(counter, &mut self.rng),
            _ => self.path.render(counter, &mut self.rng),
        };
        let mut headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(name, value)| (name.clone(), value.render(counter, &mut self.rng)))
            .collect();
        if let (Some(capture), Some(value)) = (&self.capture, captured) {
            headers.push((capture.to.clone(), value));
        }
        let body = self
            .body
            .as_ref()
            .map(|body| body.render(counter, &mut self.rng).into_bytes());
        format_request(
            &self.method,
            &path,
            &overlay(base, &headers),
            body.as_deref(),
            Some(&self.host),
        )
    }
}

/// Registry factory that builds a [`ConfigEngine`].
pub fn factory(spec: &ScriptSpec) -> Result<Box<dyn ScriptEngine>, EngineError> {
    let engine = ConfigEngine::create(spec)?;
    Ok(Box::new(engine))
}

/// Parses one file field into a template, errors keep the field.
fn template_of(field: &str, source: &str) -> Result<Template, EngineError> {
    Template::new(source)
        .map_err(|error| EngineError::Load(format!("invalid template in {field}: {error}")))
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
        let (request, dynamic) = if file.is_dynamic() {
            let capture = match &file.capture {
                Some(rule) => Some(DynamicCapture {
                    header: rule.header.clone(),
                    to: rule.to.clone().unwrap_or_else(|| rule.header.clone()),
                    path: rule
                        .path
                        .as_deref()
                        .map(|path| template_of("capture.path", path))
                        .transpose()?,
                    value: None,
                }),
                None => None,
            };
            let dynamic = DynamicRequest {
                method: file
                    .request
                    .method
                    .clone()
                    .unwrap_or_else(|| "GET".to_owned()),
                path: match &file.request.path {
                    Some(path) => template_of("request.path", path)?,
                    // The URL path is not a file field, it stays
                    // literal even beside templated fields.
                    None => Template::literal(&spec.parts.path),
                },
                headers: file
                    .request
                    .headers
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), template_of("request.headers", value)?)))
                    .collect::<Result<Vec<_>, EngineError>>()?,
                body: file
                    .request
                    .body
                    .as_ref()
                    .map(|body| template_of("request.body", body))
                    .transpose()?,
                host,
                counter: 1,
                rng: Rng::new(),
                capture,
            };
            (Vec::new(), Some(dynamic))
        } else {
            let request = format_request(
                file.request.method.as_deref().unwrap_or("GET"),
                file.request.path.as_deref().unwrap_or(&spec.parts.path),
                &overlay(&spec.headers, &file.request.headers),
                file.request.body.as_deref().map(str::as_bytes),
                Some(&host),
            );
            (request, None)
        };
        Ok(ConfigEngine {
            request,
            dynamic,
            base_headers: spec.headers.clone(),
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
        match &mut self.dynamic {
            Some(dynamic) => Ok(dynamic.render(&self.base_headers)),
            None => Ok(self.request.clone()),
        }
    }

    fn response(
        &mut self,
        _status: u16,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(), EngineError> {
        self.completed += 1;
        self.received += body.len() as u64;
        if let Some(dynamic) = &mut self.dynamic
            && let Some(capture) = &mut dynamic.capture
        {
            capture.offer(headers);
        }
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
            is_static: self.dynamic.is_none(),
            wants_response: self.file.stop.is_set() || self.file.capture.is_some(),
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
    fn a_configured_path_lands_in_the_request_line() {
        let script = temp_file(
            "wrkrs-config-path.toml",
            "[request]\npath = \"/api?key=1&x=2\"\n",
        );
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), vec![])).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET /api?key=1&x=2 HTTP/1.1\r\nHost: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn a_configured_method_passes_through_untouched() {
        let script = temp_file("wrkrs-config-method.toml", "[request]\nmethod = \"get\"\n");
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), vec![])).unwrap();
        assert!(engine.request().unwrap().starts_with(b"get / HTTP/1.1\r\n"));
    }

    #[test]
    fn a_configured_host_header_replaces_the_default() {
        let script = temp_file(
            "wrkrs-config-host.toml",
            "[request.headers]\nHost = \"proxy.example.test\"\n",
        );
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), vec![])).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET / HTTP/1.1\r\nHost: proxy.example.test\r\n\r\n"
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

    #[test]
    fn a_templated_file_renders_per_request_and_reports_dynamic() {
        let script = temp_file(
            "wrkrs-config-template.toml",
            "[request]\npath = \"/items/{{n}}\"\nbody = \"n={{n}}\"\n\
             [request.headers]\nX-Counter = \"{{n}}\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        let capabilities = engine.capabilities();
        assert!(!capabilities.is_static);
        assert!(!capabilities.wants_response);
        assert_eq!(
            engine.request().unwrap(),
            b"GET /items/1 HTTP/1.1\r\n\
              X-Counter: 1\r\n\
              Host: example.test:8080\r\n\
              Content-Length: 3\r\n\
              \r\n\
              n=1"
        );
        assert_eq!(
            engine.request().unwrap(),
            b"GET /items/2 HTTP/1.1\r\n\
              X-Counter: 2\r\n\
              Host: example.test:8080\r\n\
              Content-Length: 3\r\n\
              \r\n\
              n=2"
        );
    }

    #[test]
    fn the_content_length_follows_the_rendered_body() {
        let script = temp_file(
            "wrkrs-config-template-body.toml",
            "[request]\nbody = \"{{n}}\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        for _ in 1..10 {
            engine.request().unwrap();
        }
        // The tenth render grows the body from one digit to two.
        assert_eq!(
            engine.request().unwrap(),
            b"GET / HTTP/1.1\r\nHost: example.test:8080\r\nContent-Length: 2\r\n\r\n10"
        );
    }

    #[test]
    fn random_tags_render_digits_that_change_per_request() {
        let script = temp_file(
            "wrkrs-config-template-rand.toml",
            "[request]\npath = \"/session/{{rand}}\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        assert!(!engine.capabilities().is_static);
        let first = engine.request().unwrap();
        let second = engine.request().unwrap();
        let target = |request: &[u8]| {
            let line_end = request
                .iter()
                .position(|byte| *byte == b'\r')
                .expect("request line ends");
            request[..line_end]
                .split(|byte| *byte == b' ')
                .nth(1)
                .expect("the request line target")
                .to_owned()
        };
        for path in [target(&first), target(&second)] {
            assert!(path.starts_with(b"/session/"));
            assert!(
                path[b"/session/".len()..]
                    .iter()
                    .all(|byte| byte.is_ascii_digit())
            );
        }
        assert_ne!(first, second);
    }

    #[test]
    fn dash_h_values_stay_literal_under_a_templated_file() {
        let script = temp_file(
            "wrkrs-config-template-overlay.toml",
            "[request]\npath = \"/{{n}}\"\n[request.headers]\nX-Counter = \"{{n}}\"\n",
        );
        let headers = vec![
            ("X-Counter".to_owned(), "static".to_owned()),
            ("User-Agent".to_owned(), "wrkrs".to_owned()),
        ];
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), headers)).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET /1 HTTP/1.1\r\n\
              X-Counter: 1\r\n\
              User-Agent: wrkrs\r\n\
              Host: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn the_url_path_stays_literal_beside_templated_fields() {
        let script = temp_file(
            "wrkrs-config-template-url-path.toml",
            "[request]\nbody = \"{{n}}\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        let request = engine.request().unwrap();
        assert!(request.starts_with(b"GET / HTTP/1.1\r\n"));
        assert!(request.ends_with(b"\r\n\r\n1"));
    }

    #[test]
    fn templated_files_json_shaped_render_the_same_way() {
        let script = temp_file(
            "wrkrs-config-template.json",
            "{\"request\":{\"path\":\"/{{n}}\",\"headers\":{\"X-Counter\":\"{{n}}\"}}}",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        assert!(!engine.capabilities().is_static);
        assert_eq!(
            engine.request().unwrap(),
            b"GET /1 HTTP/1.1\r\nX-Counter: 1\r\nHost: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn templated_files_keep_stop_conditions() {
        let script = temp_file(
            "wrkrs-config-template-stop.toml",
            "[request]\npath = \"/{{n}}\"\n[stop]\nrequests = 2\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        let capabilities = engine.capabilities();
        assert!(!capabilities.is_static);
        assert!(capabilities.wants_response);
        let (thread, handle) = stopped_thread();
        engine.init(handle, &[]).unwrap();
        engine.request().unwrap();
        engine.response(200, &[], b"ok").unwrap();
        assert!(!thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
        engine.response(200, &[], b"ok").unwrap();
        assert!(thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn each_engine_instance_counts_its_own_counter() {
        let script = temp_file(
            "wrkrs-config-template-threads.toml",
            "[request]\npath = \"/{{n}}\"\n",
        );
        // One engine per thread, the runner builds them from one
        // spec, so the counters start apart.
        let mut first = ConfigEngine::create(&spec(&script)).unwrap();
        let mut second = ConfigEngine::create(&spec(&script)).unwrap();
        assert!(first.request().unwrap().starts_with(b"GET /1 "));
        assert!(second.request().unwrap().starts_with(b"GET /1 "));
        assert!(first.request().unwrap().starts_with(b"GET /2 "));
    }

    #[test]
    fn a_capture_rule_runs_the_auth_flow() {
        let script = temp_file(
            "wrkrs-config-capture.toml",
            "[request]\npath = \"/authenticate\"\n[capture]\nheader = \"X-Token\"\n\
             path = \"/resource\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        let capabilities = engine.capabilities();
        assert!(!capabilities.is_static);
        assert!(capabilities.wants_response);
        assert_eq!(
            engine.request().unwrap(),
            b"GET /authenticate HTTP/1.1\r\nHost: example.test:8080\r\n\r\n"
        );
        engine
            .response(200, &[("X-Token".to_owned(), "abc123".to_owned())], b"ok")
            .unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET /resource HTTP/1.1\r\n\
              X-Token: abc123\r\n\
              Host: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn responses_without_the_header_keep_the_auth_phase() {
        let script = temp_file(
            "wrkrs-config-capture-wait.toml",
            "[request]\npath = \"/authenticate\"\n[capture]\nheader = \"X-Token\"\n\
             path = \"/resource\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        engine.response(503, &[], b"down").unwrap();
        assert!(engine.request().unwrap().starts_with(b"GET /authenticate "));
        engine
            .response(
                200,
                &[("Content-Type".to_owned(), "text/plain".to_owned())],
                b"",
            )
            .unwrap();
        assert!(engine.request().unwrap().starts_with(b"GET /authenticate "));
    }

    #[test]
    fn the_header_matches_case_insensitively() {
        let script = temp_file(
            "wrkrs-config-capture-case.toml",
            "[capture]\nheader = \"X-Token\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        engine
            .response(200, &[("x-token".to_owned(), "v".to_owned())], b"")
            .unwrap();
        assert!(contains(&engine.request().unwrap(), b"\r\nX-Token: v\r\n"));
    }

    #[test]
    fn the_capture_lands_once_and_never_refreshes() {
        let script = temp_file(
            "wrkrs-config-capture-once.toml",
            "[capture]\nheader = \"X-Token\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        engine
            .response(200, &[("X-Token".to_owned(), "first".to_owned())], b"")
            .unwrap();
        engine
            .response(200, &[("X-Token".to_owned(), "second".to_owned())], b"")
            .unwrap();
        assert!(contains(
            &engine.request().unwrap(),
            b"\r\nX-Token: first\r\n"
        ));
    }

    #[test]
    fn a_capture_target_renames_the_header() {
        let script = temp_file(
            "wrkrs-config-capture-rename.toml",
            "[capture]\nheader = \"X-Token\"\nto = \"Authorization\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        engine
            .response(200, &[("X-Token".to_owned(), "abc".to_owned())], b"")
            .unwrap();
        assert!(contains(
            &engine.request().unwrap(),
            b"\r\nAuthorization: abc\r\n"
        ));
    }

    #[test]
    fn without_a_capture_path_the_requests_stay_on_theirs() {
        let script = temp_file(
            "wrkrs-config-capture-same-path.toml",
            "[request]\npath = \"/api\"\n[capture]\nheader = \"Set-Cookie\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        engine
            .response(200, &[("Set-Cookie".to_owned(), "id=1".to_owned())], b"")
            .unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET /api HTTP/1.1\r\nSet-Cookie: id=1\r\nHost: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn the_captured_header_layers_over_dash_h_and_file_values() {
        let script = temp_file(
            "wrkrs-config-capture-overlay.toml",
            "[request]\npath = \"/authenticate\"\n\
             [request.headers]\nX-Token = \"stale\"\n\
             [capture]\nheader = \"X-Token\"\npath = \"/resource\"\n",
        );
        let headers = vec![
            ("X-Token".to_owned(), "dash-h".to_owned()),
            ("User-Agent".to_owned(), "wrkrs".to_owned()),
        ];
        let mut engine = ConfigEngine::create(&spec_with(Some(&script), headers)).unwrap();
        assert!(contains(
            &engine.request().unwrap(),
            b"\r\nX-Token: stale\r\n"
        ));
        engine
            .response(200, &[("X-Token".to_owned(), "fresh".to_owned())], b"")
            .unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET /resource HTTP/1.1\r\n\
              X-Token: fresh\r\n\
              User-Agent: wrkrs\r\n\
              Host: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn templated_files_capture_the_rendered_flow() {
        let script = temp_file(
            "wrkrs-config-capture-template.toml",
            "[request]\npath = \"/authenticate\"\n[capture]\nheader = \"X-Token\"\n\
             path = \"/items/{{n}}\"\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        engine.request().unwrap();
        engine
            .response(200, &[("X-Token".to_owned(), "abc".to_owned())], b"")
            .unwrap();
        // The captured phase renders its own template, starting from
        // the counter it left off at.
        assert_eq!(
            engine.request().unwrap(),
            b"GET /items/2 HTTP/1.1\r\nX-Token: abc\r\nHost: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn capture_rules_keep_stop_conditions() {
        let script = temp_file(
            "wrkrs-config-capture-stop.toml",
            "[request]\npath = \"/authenticate\"\n[capture]\nheader = \"X-Token\"\n\
             path = \"/resource\"\n[stop]\nrequests = 2\n",
        );
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        let capabilities = engine.capabilities();
        assert!(!capabilities.is_static);
        assert!(capabilities.wants_response);
        let (thread, handle) = stopped_thread();
        engine.init(handle, &[]).unwrap();
        engine
            .response(200, &[("X-Token".to_owned(), "abc".to_owned())], b"ok")
            .unwrap();
        assert!(!thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
        engine.response(200, &[], b"ok").unwrap();
        assert!(thread.stopped.load(std::sync::atomic::Ordering::Relaxed));
        assert!(engine.request().unwrap().starts_with(b"GET /resource "));
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

    /// Whether the request bytes carry the byte slice.
    fn contains(bytes: &[u8], needle: &[u8]) -> bool {
        bytes.windows(needle.len()).any(|window| window == needle)
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
    fn responses_before_init_count_without_a_thread() {
        let script = temp_file("wrkrs-config-stop-early.toml", "[stop]\nrequests = 1\n");
        let mut engine = ConfigEngine::create(&spec(&script)).unwrap();
        engine.response(200, &[], b"ok").expect("no thread yet");
        let (thread, handle) = stopped_thread();
        engine.init(handle, &[]).unwrap();
        engine.response(200, &[], b"ok").unwrap();
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
