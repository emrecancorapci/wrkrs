use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use wrkrs_engine::{
    Capabilities, EngineError, ResolveApi, ScriptEngine, ScriptSpec, StatsView, Summary, ThreadApi,
    Value, format_request, host_header,
};

/// Minimal engine used to exercise the registry without a real VM.
///
/// Produces the default request for the spec, stores globals in a map,
/// and accepts every callback as a no-op. Conformance behavior lives in
/// the real engines, this one only proves the plumbing.
pub struct StubEngine {
    request: Vec<u8>,
    globals: HashMap<String, Value>,
    thread: Option<Arc<dyn ThreadApi>>,
}

/// Registry factory that builds a [`StubEngine`].
pub fn factory(spec: &ScriptSpec) -> Result<Box<dyn ScriptEngine>, EngineError> {
    let engine = StubEngine::create(spec)?;
    Ok(Box::new(engine))
}

impl ScriptEngine for StubEngine {
    fn create(spec: &ScriptSpec) -> Result<Self, EngineError> {
        let host = host_header(
            spec.parts.host.as_deref().unwrap_or_default(),
            spec.parts.port.as_deref(),
        );
        let request = format_request("GET", &spec.parts.path, &spec.headers, None, Some(&host));
        Ok(StubEngine {
            request,
            globals: HashMap::new(),
            thread: None,
        })
    }

    fn resolve(
        &mut self,
        host: &str,
        service: &str,
        resolver: &dyn ResolveApi,
    ) -> Result<Vec<SocketAddr>, EngineError> {
        let addresses = resolver
            .lookup(host, service)
            .map_err(|error| EngineError::Resolve(error.to_string()))?;
        Ok(addresses
            .into_iter()
            .filter(|address| resolver.connect(address))
            .collect())
    }

    fn setup(&mut self, _thread: &dyn ThreadApi) -> Result<(), EngineError> {
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
        _body: &[u8],
    ) -> Result<(), EngineError> {
        Ok(())
    }

    fn done(
        &mut self,
        _summary: &Summary,
        _latency: &dyn StatsView,
        _requests: &dyn StatsView,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            is_static: true,
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
