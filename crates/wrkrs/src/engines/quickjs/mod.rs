//! The QuickJS scripting engine.
//!
//! Ports the wrk scripting surface to JavaScript. The wrk object and
//! the callbacks follow the Lua engine semantics, with the differences
//! documented in ENGINES.md.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use rquickjs::{Context, IntoJs, Runtime};
use wrkrs_engine::{
    Capabilities, EngineError, ResolveApi, ScriptEngine, ScriptSpec, StatsView, Summary, ThreadApi,
    Value,
};

use self::address::AddressObject;

/// Shared slot holding the host resolver the lookup functions use.
type ResolverSlot = Arc<Mutex<Option<Arc<dyn ResolveApi>>>>;

/// Heap ceiling for one scripting environment.
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;

/// Native stack ceiling for one scripting environment.
const STACK_LIMIT: usize = 1024 * 1024;

/// One QuickJS scripting environment driven by the host.
pub struct QuickJSEngine {
    context: Context,
    resolver: ResolverSlot,
}

impl QuickJSEngine {
    /// Builds one environment from a spec.
    pub fn new(spec: &ScriptSpec) -> Result<Self, EngineError> {
        // The context keeps the runtime alive on its own.
        let context = Runtime::new()
            .map_err(|error| EngineError::Runtime(error.to_string()))
            .and_then(|runtime| {
                // Guard rails against runaway scripts: a script that
                // exhausts either limit fails that call instead of
                // taking the process down.
                runtime.set_memory_limit(MEMORY_LIMIT);
                runtime.set_max_stack_size(STACK_LIMIT);
                Context::full(&runtime).map_err(|error| EngineError::Runtime(error.to_string()))
            })?;
        let engine = QuickJSEngine {
            context,
            resolver: Arc::new(Mutex::new(None)),
        };
        engine.with(|ctx| install_wrk(ctx, spec))?;
        engine.with(|ctx| {
            thread::define(ctx)?;
            stats::define(ctx)?;
            install_lookup(ctx, &engine.resolver)
        })?;
        engine.run_script_file(spec.script.as_deref());
        Ok(engine)
    }

    /// Runs the script file for an environment.
    ///
    /// Load failures print `path: message` on stderr and the run keeps
    /// the default behavior, matching script.c.
    fn run_script_file(&self, script: Option<&std::path::Path>) {
        let Some(path) = script else {
            return;
        };
        let outcome = match std::fs::read_to_string(path) {
            Ok(source) => self.with(|ctx| ctx.eval::<(), _>(source.as_str())),
            Err(error) => Err(EngineError::Runtime(format!(
                "cannot open {}: {error}",
                path.display()
            ))),
        };
        if let Err(error) = &outcome {
            eprintln!("{}: {}", path.display(), error.raw_message());
        }
    }

    /// Runs a closure inside the context with engine error mapping.
    fn with<F, R>(&self, function: F) -> Result<R, EngineError>
    where
        F: FnOnce(&rquickjs::Ctx<'_>) -> Result<R, rquickjs::Error>,
    {
        self.context
            .with(|ctx| function(&ctx).map_err(|error| ctx_error(&ctx, error)))
    }
}

/// Maps an in-context error, carrying the exception message when the
/// VM raised one.
fn ctx_error(ctx: &rquickjs::Ctx<'_>, error: rquickjs::Error) -> EngineError {
    let message = if matches!(error, rquickjs::Error::Exception) {
        exception_message(ctx)
    } else {
        error.to_string()
    };
    EngineError::Runtime(message)
}

/// Reads the message of the pending JavaScript exception.
pub(crate) fn exception_message(ctx: &rquickjs::Ctx<'_>) -> String {
    let exception = ctx.catch();
    if let Some(object) = exception.into_object()
        && let Some(error) = rquickjs::Exception::from_object(object)
        && let Some(message) = error.message()
    {
        return message;
    }
    "script exception".to_owned()
}

/// Creates the wrk object with the URL parts and headers.
fn install_wrk(ctx: &rquickjs::Ctx<'_>, spec: &ScriptSpec) -> Result<(), rquickjs::Error> {
    let headers = rquickjs::Object::new(ctx.clone())?;
    for (name, value) in &spec.headers {
        headers.set(name.as_str(), value.as_str())?;
    }

    let wrk = rquickjs::Object::new(ctx.clone())?;
    wrk.set("scheme", part_value(ctx, &spec.parts.scheme)?)?;
    wrk.set("host", part_value(ctx, &spec.parts.host)?)?;
    wrk.set("port", part_value(ctx, &spec.parts.port)?)?;
    wrk.set("method", "GET")?;
    wrk.set("path", spec.parts.path.as_str())?;
    wrk.set("headers", headers)?;
    wrk.set("body", rquickjs::Value::new_null(ctx.clone()))?;
    wrk.set("thread", rquickjs::Value::new_null(ctx.clone()))?;

    let format = rquickjs::Function::new(ctx.clone(), format_request_js)?;
    wrk.set("format", format)?;
    ctx.globals().set("wrk", wrk)?;

    let print = rquickjs::Function::new(ctx.clone(), print_js)?;
    ctx.globals().set("print", print)?;
    Ok(())
}

/// Prints a line to stdout, the console stand in for scripts.
fn print_js(text: rquickjs::function::Opt<String>) {
    println!("{}", text.0.unwrap_or_default());
}

/// Backs wrk.format with the core formatter.
///
/// Mirrors the wrk.lua mutations: a missing Host comes from the default
/// headers and Content-Length lands on the passed headers object, or is
/// removed when no body is given.
fn format_request_js<'js>(
    ctx: rquickjs::Ctx<'js>,
    method: rquickjs::function::Opt<Option<String>>,
    path: rquickjs::function::Opt<Option<String>>,
    headers: rquickjs::function::Opt<Option<rquickjs::Object<'js>>>,
    body: rquickjs::function::Opt<Option<String>>,
) -> Result<String, rquickjs::Error> {
    let wrk: rquickjs::Object = ctx.globals().get("wrk")?;
    let default_headers: rquickjs::Object = wrk.get("headers")?;
    let headers = match headers.0 {
        Some(Some(headers)) => headers,
        _ => default_headers.clone(),
    };

    // A missing or null argument means the wrk default, matching the
    // or-fallbacks of wrk.lua.
    let method = match method.0.flatten() {
        Some(method) => method,
        None => wrk.get("method")?,
    };
    let path = match path.0.flatten() {
        Some(path) => path,
        None => wrk.get("path")?,
    };
    let body = match body.0.flatten() {
        Some(body) => Some(body),
        None => wrk.get("body")?,
    };

    let mut pairs: Vec<(String, String)> = Vec::new();
    for key in headers.keys::<String>() {
        let key = key?;
        let value: rquickjs::Value = headers.get(key.as_str())?;
        pairs.push((key, coerce_scalar(&value)?));
    }
    let default_host: Option<String> = default_headers
        .get::<_, rquickjs::Value>("Host")
        .ok()
        .and_then(|value| coerce_scalar(&value).ok());
    let has_host = pairs.iter().any(|(name, _)| name == "Host");

    let request = wrkrs_engine::format_request(
        &method,
        &path,
        &pairs,
        body.as_deref().map(str::as_bytes),
        default_host.as_deref(),
    );

    // Write the changes back the way wrk.lua mutates its tables.
    if !has_host && let Some(host) = default_host {
        headers.set("Host", host)?;
    }
    match &body {
        Some(body) => headers.set("Content-Length", body.len())?,
        None => {
            let _ = headers.remove("Content-Length");
        }
    }

    String::from_utf8(request).map_err(|error| rquickjs::Error::FromJs {
        from: "request",
        to: "string",
        message: Some(error.to_string()),
    })
}

mod address;
mod stats;
mod thread;
mod value;

/// Presents an absent part as null, matching the Lua nil.
fn part_value<'js>(
    ctx: &rquickjs::Ctx<'js>,
    part: &Option<String>,
) -> Result<rquickjs::Value<'js>, rquickjs::Error> {
    match part {
        Some(value) => value.clone().into_js(ctx),
        None => Ok(rquickjs::Value::new_null(ctx.clone())),
    }
}

/// Coerces a scalar header value to a string the way Lua's %s does.
fn coerce_scalar(value: &rquickjs::Value<'_>) -> Result<String, rquickjs::Error> {
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    if let Some(int) = value.as_int() {
        return Ok(int.to_string());
    }
    if let Some(float) = value.as_float() {
        return Ok(float.to_string());
    }
    if let Some(boolean) = value.as_bool() {
        return Ok(boolean.to_string());
    }
    Err(rquickjs::Error::FromJs {
        from: "value",
        to: "string",
        message: Some("only scalars convert".to_owned()),
    })
}

/// Holds the shared resolver for the lookup functions on a context.
#[rquickjs::class(rename_all = "camelCase")]
pub(crate) struct ResolverHolder {
    /// The engine resolver slot.
    pub slot: ResolverSlot,
}

impl<'js> rquickjs::class::Trace<'js> for ResolverHolder {
    fn trace<'a>(&self, _tracer: rquickjs::class::Tracer<'a, 'js>) {}
}

unsafe impl<'js> rquickjs::JsLifetime<'js> for ResolverHolder {
    type Changed<'to> = Self;
}

#[rquickjs::methods]
impl ResolverHolder {
    fn lookup<'js>(
        &self,
        ctx: rquickjs::Ctx<'js>,
        host: String,
        service: String,
    ) -> Result<rquickjs::Array<'js>, rquickjs::Error> {
        let resolver = shared_resolver(&ctx, &self.slot)?;
        let addresses = resolver.lookup(&host, &service).map_err(|error| {
            rquickjs::Exception::throw_message(
                &ctx,
                &format!("unable to resolve {host}:{service} {error}"),
            )
        })?;
        let array = rquickjs::Array::new(ctx.clone())?;
        for (index, address) in addresses.iter().enumerate() {
            let instance = self::address::instance(ctx.clone(), *address)?;
            array.set(index, instance)?;
        }
        Ok(array)
    }

    fn connect<'js>(
        &self,
        ctx: rquickjs::Ctx<'js>,
        address: rquickjs::Class<'js, AddressObject>,
    ) -> Result<bool, rquickjs::Error> {
        let resolver = shared_resolver(&ctx, &self.slot)?;
        let address = address.try_borrow()?;
        Ok(resolver.connect(&address.address))
    }
}

/// Installs wrk.lookup and wrk.connect into the wrk object.
///
/// The functions read the shared resolver slot, which the host fills
/// when resolve runs. They bind to a hidden holder instance so script
/// calls keep the right receiver.
fn install_lookup(ctx: &rquickjs::Ctx<'_>, slot: &ResolverSlot) -> Result<(), rquickjs::Error> {
    rquickjs::Class::<ResolverHolder>::define(&ctx.globals())?;
    let holder = rquickjs::Class::<ResolverHolder>::instance(
        ctx.clone(),
        ResolverHolder { slot: slot.clone() },
    )?;
    ctx.globals().set("__wrkrsHolder", holder)?;
    ctx.eval::<(), _>(
        "wrk.lookup = __wrkrsHolder.lookup.bind(__wrkrsHolder)\n\
         wrk.connect = __wrkrsHolder.connect.bind(__wrkrsHolder)\n\
         delete globalThis.__wrkrsHolder",
    )?;
    Ok(())
}

/// Takes the current resolver for a lookup function call.
fn shared_resolver(
    ctx: &rquickjs::Ctx<'_>,
    slot: &ResolverSlot,
) -> Result<Arc<dyn ResolveApi>, rquickjs::Error> {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or_else(|| {
            rquickjs::Exception::throw_message(
                ctx,
                "wrk.lookup is not available before the run resolves",
            )
        })
}

/// Registry factory that builds a [`QuickJSEngine`].
pub fn factory(spec: &ScriptSpec) -> Result<Box<dyn ScriptEngine>, EngineError> {
    Ok(Box::new(QuickJSEngine::new(spec)?))
}

impl ScriptEngine for QuickJSEngine {
    fn create(spec: &ScriptSpec) -> Result<Self, EngineError>
    where
        Self: Sized,
    {
        QuickJSEngine::new(spec)
    }

    fn resolve(
        &mut self,
        host: &str,
        service: &str,
        resolver: Arc<dyn ResolveApi>,
    ) -> Result<Vec<SocketAddr>, EngineError> {
        *self
            .resolver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(resolver);
        self.with(|ctx| {
            let wrk: rquickjs::Object = ctx.globals().get("wrk")?;
            let lookup: rquickjs::Function = wrk.get("lookup")?;
            let connect: rquickjs::Function = wrk.get("connect")?;
            let candidates: rquickjs::Array = lookup.call((host.to_owned(), service.to_owned()))?;

            // The wrk.resolve step: keep the addresses that accept a
            // connection probe, in resolver order.
            let mut reachable = Vec::new();
            let filtered = rquickjs::Array::new(ctx.clone())?;
            for entry in candidates.iter::<rquickjs::Class<'_, AddressObject>>() {
                let candidate = entry?;
                let address = candidate.clone().try_borrow()?.address;
                if connect.call::<_, bool>((candidate.clone(),))? {
                    filtered.set(reachable.len(), candidate)?;
                    reachable.push(address);
                }
            }
            wrk.set("addrs", filtered)?;
            Ok(reachable)
        })
    }

    fn setup(&mut self, thread: Arc<dyn ThreadApi>) -> Result<(), EngineError> {
        self.with(|ctx| {
            let instance = thread::instance(ctx.clone(), thread.clone())?;
            // The wrk.setup step: point the thread at the first address.
            let wrk: rquickjs::Object = ctx.globals().get("wrk")?;
            if let Ok(addrs) = wrk.get::<_, rquickjs::Array>("addrs")
                && let Ok(first) = addrs.get::<rquickjs::Class<'_, AddressObject>>(0)
            {
                let address = first.try_borrow()?;
                thread.set_addr(address.address);
            }
            let setup: Option<rquickjs::Function> = ctx.globals().get("setup")?;
            if let Some(setup) = setup {
                setup.call::<_, ()>((instance,))?;
            }
            Ok(())
        })
    }

    fn init(&mut self, thread: Arc<dyn ThreadApi>, args: &[String]) -> Result<(), EngineError> {
        self.with(|ctx| {
            let instance = thread::instance(ctx.clone(), thread)?;
            let wrk: rquickjs::Object = ctx.globals().get("wrk")?;
            wrk.set("thread", instance)?;

            // The wrk.init step: a missing Host header comes from the
            // host and port parts.
            let headers: rquickjs::Object = wrk.get("headers")?;
            let has_host = headers.get::<_, Option<String>>("Host")?.is_some();
            if !has_host {
                let host: Option<String> = wrk.get("host")?;
                let port: Option<String> = wrk.get("port")?;
                if let Some(host) = host {
                    headers.set("Host", wrkrs_engine::host_header(&host, port.as_deref()))?;
                }
            }

            let arguments = rquickjs::Array::new(ctx.clone())?;
            for (index, arg) in args.iter().enumerate() {
                arguments.set(index, arg.clone())?;
            }
            let init: Option<rquickjs::Function> = ctx.globals().get("init")?;
            if let Some(init) = init {
                init.call::<_, ()>((arguments,))?;
            }
            Ok(())
        })
    }

    fn delay(&mut self) -> u64 {
        let result = self.with(|ctx| {
            let delay: Option<rquickjs::Function> = ctx.globals().get("delay")?;
            match delay {
                Some(delay) => delay.call::<_, f64>(()),
                None => Ok(0.0),
            }
        });
        match result {
            Ok(delay) => delay as u64,
            Err(error) => {
                // wrk calls delay unprotected, a script error aborts the
                // whole run.
                eprintln!("{}", error.raw_message());
                std::process::exit(1);
            }
        }
    }

    fn request(&mut self) -> Result<Vec<u8>, EngineError> {
        self.with(|ctx| {
            let request: Option<rquickjs::Function> = ctx.globals().get("request")?;
            match request {
                Some(request) => {
                    let value: rquickjs::Value = request.call(())?;
                    match request_bytes(&value) {
                        Some(bytes) => Ok(bytes),
                        None => Err(rquickjs::Exception::throw_message(
                            ctx,
                            "request must return a string or a number",
                        )),
                    }
                }
                None => {
                    let wrk: rquickjs::Object = ctx.globals().get("wrk")?;
                    let format: rquickjs::Function = wrk.get("format")?;
                    let request: String = format.call(())?;
                    Ok(request.into_bytes())
                }
            }
        })
    }

    fn response(
        &mut self,
        status: u16,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(), EngineError> {
        self.with(|ctx| {
            let object = rquickjs::Object::new(ctx.clone())?;
            for (name, value) in headers {
                object.set(name.as_str(), value.as_str())?;
            }
            let body = String::from_utf8_lossy(body).to_string();
            let response: rquickjs::Function = ctx.globals().get("response")?;
            response.call::<_, ()>((status, object, body))
        })
    }

    fn done(
        &mut self,
        summary: &Summary,
        latency: Arc<dyn StatsView>,
        requests: Arc<dyn StatsView>,
    ) -> Result<(), EngineError> {
        self.with(|ctx| {
            let errors = rquickjs::Object::new(ctx.clone())?;
            errors.set("connect", summary.errors.connect)?;
            errors.set("read", summary.errors.read)?;
            errors.set("write", summary.errors.write)?;
            errors.set("status", summary.errors.status)?;
            errors.set("timeout", summary.errors.timeout)?;

            let summary_object = rquickjs::Object::new(ctx.clone())?;
            summary_object.set("duration", summary.duration)?;
            summary_object.set("requests", summary.requests)?;
            summary_object.set("bytes", summary.bytes)?;
            summary_object.set("errors", errors)?;

            let latency = stats::instance(ctx.clone(), latency)?;
            let requests = stats::instance(ctx.clone(), requests)?;
            let done: rquickjs::Function = ctx.globals().get("done")?;
            done.call::<_, ()>((summary_object, latency, requests))
        })
    }

    fn capabilities(&self) -> Capabilities {
        let Ok((request, response, delay, done)) = self.with(|ctx| {
            Ok((
                ctx.globals()
                    .get::<_, rquickjs::Function>("request")
                    .is_ok(),
                ctx.globals()
                    .get::<_, rquickjs::Function>("response")
                    .is_ok(),
                ctx.globals().get::<_, rquickjs::Function>("delay").is_ok(),
                ctx.globals().get::<_, rquickjs::Function>("done").is_ok(),
            ))
        }) else {
            return Capabilities::default();
        };
        Capabilities {
            is_static: !request,
            wants_response: response,
            has_delay: delay,
            has_done: done,
        }
    }

    fn get_global(&self, name: &str) -> Result<Value, EngineError> {
        self.with(
            |ctx| -> Result<Result<Value, EngineError>, rquickjs::Error> {
                let value: rquickjs::Value = ctx.globals().get(name)?;
                Ok(value::js_to_value(&value))
            },
        )?
    }

    fn set_global(&mut self, name: &str, value: &Value) -> Result<(), EngineError> {
        self.with(|ctx| {
            let converted = value::value_to_js(ctx, value)?;
            ctx.globals().set(name, converted)
        })
    }
}

/// Converts a request callback return value into bytes.
fn request_bytes(value: &rquickjs::Value<'_>) -> Option<Vec<u8>> {
    if let Some(string) = value.as_string() {
        return string.to_string().ok().map(String::into_bytes);
    }
    if let Some(int) = value.as_int() {
        return Some(int.to_string().into_bytes());
    }
    if let Some(float) = value.as_float() {
        return Some(float.to_string().into_bytes());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::QuickJSEngine;
    use wrkrs_engine::Value;
    use wrkrs_engine_tests::fixtures::spec;

    #[test]
    fn evaluates_javascript() {
        let engine = QuickJSEngine::new(&spec(None)).unwrap();
        let result: i32 = engine.with(|ctx| ctx.eval("1 + 1")).unwrap();
        assert_eq!(result, 2);
    }

    #[test]
    fn reports_script_exception_messages() {
        let engine = QuickJSEngine::new(&spec(None)).unwrap();
        let error = engine
            .with(|ctx| ctx.eval::<i32, _>("throw new Error(\"boom\")"))
            .unwrap_err();
        assert_eq!(error.raw_message(), "boom");
    }

    #[test]
    fn builds_the_wrk_object_from_the_spec() {
        let engine = QuickJSEngine::new(&spec(None)).unwrap();
        let host: String = engine.with(|ctx| ctx.eval("wrk.host")).unwrap();
        let port: String = engine.with(|ctx| ctx.eval("wrk.port")).unwrap();
        let method: String = engine.with(|ctx| ctx.eval("wrk.method")).unwrap();
        let path: String = engine.with(|ctx| ctx.eval("wrk.path")).unwrap();
        assert_eq!(host, "example.test");
        assert_eq!(port, "8080");
        assert_eq!(method, "GET");
        assert_eq!(path, "/some/path");
    }

    #[test]
    fn applies_command_line_headers() {
        let mut given = spec(None);
        given.headers = vec![("Accept".to_owned(), "application/json".to_owned())];
        let engine = QuickJSEngine::new(&given).unwrap();
        let accept: String = engine.with(|ctx| ctx.eval("wrk.headers.Accept")).unwrap();
        assert_eq!(accept, "application/json");
    }

    #[test]
    fn format_matches_the_core_formatter() {
        let engine = QuickJSEngine::new(&spec(None)).unwrap();
        engine
            .with(|ctx| ctx.eval::<(), _>("wrk.headers.Host = \"example.test:8080\""))
            .unwrap();
        let request: String = engine.with(|ctx| ctx.eval("wrk.format()")).unwrap();
        assert_eq!(
            request,
            "GET /some/path HTTP/1.1\r\nHost: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn format_takes_method_path_headers_and_body() {
        let engine = QuickJSEngine::new(&spec(None)).unwrap();
        let request: String = engine
            .with(|ctx| {
                ctx.eval(
                    "wrk.format(\"POST\", \"/x\", \
                     {\"Content-Type\": \"text/plain\"}, \"hi\")",
                )
            })
            .unwrap();
        assert_eq!(
            request,
            "POST /x HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nhi"
        );
    }

    #[test]
    fn format_writes_host_and_length_back() {
        let engine = QuickJSEngine::new(&spec(None)).unwrap();
        engine
            .with(|ctx| ctx.eval::<(), _>("wrk.headers.Host = \"example.test:8080\""))
            .unwrap();
        let written: String = engine
            .with(|ctx| {
                ctx.eval(
                    "let h = {}\n\
                     wrk.format(\"POST\", \"/\", h, \"xx\");\n\
                     [h.Host, h[\"Content-Length\"]].join(\"|\")",
                )
            })
            .unwrap();
        assert_eq!(written, "example.test:8080|2");
    }

    #[test]
    fn format_removes_length_without_a_body() {
        let engine = QuickJSEngine::new(&spec(None)).unwrap();
        let removed: bool = engine
            .with(|ctx| {
                ctx.eval(
                    "let h = {\"Content-Length\": 9}\n\
                     wrk.format(\"GET\", \"/\", h, null);\n\
                     !(\"Content-Length\" in h)",
                )
            })
            .unwrap();
        assert!(removed);
    }

    #[test]
    fn script_files_run_and_can_change_the_wrk_object() {
        let path = std::env::temp_dir().join("wrkrs-quickjs-method");
        std::fs::write(&path, "wrk.method = \"POST\"\n").expect("write temporary script");
        let engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        let method: String = engine.with(|ctx| ctx.eval("wrk.method")).unwrap();
        assert_eq!(method, "POST");
    }

    #[test]
    fn script_let_bindings_survive_for_callbacks() {
        let path = std::env::temp_dir().join("wrkrs-quickjs-let");
        std::fs::write(&path, "let counter = 1\n").expect("write temporary script");
        let engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        let counter: i32 = engine.with(|ctx| ctx.eval("counter")).unwrap();
        assert_eq!(counter, 1);
    }

    #[test]
    fn script_load_errors_report_and_continue() {
        let path = std::env::temp_dir().join("wrkrs-quickjs-broken");
        std::fs::write(&path, "this is not javascript\n").expect("write temporary script");
        let engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        let method: String = engine.with(|ctx| ctx.eval("wrk.method")).unwrap();
        assert_eq!(method, "GET");
    }

    #[test]
    fn every_example_script_loads() {
        use wrkrs_engine::ScriptEngine;

        let scripts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts");
        let entries =
            // A read failure means the test tree is not in place, which
            // the test cannot work around.
            std::fs::read_dir(scripts).expect("read the scripts directory");
        let mut loaded = 0;
        for entry in entries {
            let path = entry.expect("read a directory entry").path();
            if path.extension().is_some_and(|extension| extension == "js") {
                let engine = QuickJSEngine::new(&spec(Some(&path)))
                    .unwrap_or_else(|error| panic!("{} failed to load: {error}", path.display()));
                let _ = engine.capabilities();
                loaded += 1;
            }
        }
        assert!(loaded >= 6, "expected the example scripts, found {loaded}");
    }

    #[test]
    fn resolve_filters_unreachable_addresses() {
        use std::sync::Arc;

        use wrkrs_engine::ScriptEngine;
        use wrkrs_engine_tests::fixtures::{FakeResolver, address};

        let mut engine = QuickJSEngine::new(&spec(None)).unwrap();
        let resolved = engine
            .resolve("example.test", "8080", Arc::new(FakeResolver::default()))
            .unwrap();
        assert_eq!(resolved, vec![address(1)]);
    }

    #[test]
    fn resolve_reports_lookup_failures() {
        use std::sync::Arc;

        use wrkrs_engine::ScriptEngine;
        use wrkrs_engine_tests::fixtures::FailingResolver;

        let mut engine = QuickJSEngine::new(&spec(None)).unwrap();
        let error = engine
            .resolve("example.test", "8080", Arc::new(FailingResolver))
            .unwrap_err();
        assert_eq!(
            error.raw_message(),
            "unable to resolve example.test:8080 name or service not known"
        );
    }

    #[test]
    fn setup_assigns_the_address_and_calls_the_script() {
        use std::sync::Arc;

        use wrkrs_engine::{ScriptEngine, ThreadApi, Value};
        use wrkrs_engine_tests::fixtures::{FakeResolver, FakeThread, address};

        let path = std::env::temp_dir().join("wrkrs-quickjs-setup");
        std::fs::write(&path, "function setup(thread) { thread.set(\"id\", 7) }\n")
            .expect("write temporary script");
        let mut engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        engine
            .resolve("example.test", "8080", Arc::new(FakeResolver::default()))
            .unwrap();
        let thread = Arc::new(FakeThread::default());
        engine.setup(thread.clone()).unwrap();
        assert_eq!(thread.addr(), Some(address(1)));
        assert_eq!(thread.get_global("id").unwrap(), Value::Int(7));
    }

    #[test]
    fn init_sets_the_thread_the_host_and_the_args() {
        use std::sync::Arc;

        use wrkrs_engine::ScriptEngine;
        use wrkrs_engine_tests::fixtures::FakeThread;

        let path = std::env::temp_dir().join("wrkrs-quickjs-init");
        std::fs::write(
            &path,
            "var seen\nfunction init(args) { seen = wrk.thread != null && args[0] }\n",
        )
        .expect("write temporary script");
        let mut engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        engine
            .init(
                Arc::new(FakeThread::default()),
                &["hello".to_owned(), "world".to_owned()],
            )
            .unwrap();
        assert_eq!(
            engine.get_global("seen").unwrap(),
            Value::Str("hello".to_owned())
        );
        let host: String = engine.with(|ctx| ctx.eval("wrk.headers.Host")).unwrap();
        assert_eq!(host, "example.test:8080");
    }

    #[test]
    fn default_request_matches_the_core_formatter() {
        use std::sync::Arc;

        use wrkrs_engine::{ScriptEngine, format_request, host_header};
        use wrkrs_engine_tests::fixtures::FakeThread;

        let mut engine = QuickJSEngine::new(&spec(None)).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        let request = engine.request().unwrap();
        let host = host_header("example.test", Some("8080"));
        let expected = format_request("GET", "/some/path", &[], None, Some(&host));
        assert_eq!(request, expected);
    }

    #[test]
    fn delay_returns_the_script_value() {
        use std::sync::Arc;

        use wrkrs_engine::ScriptEngine;
        use wrkrs_engine_tests::fixtures::FakeThread;

        let path = std::env::temp_dir().join("wrkrs-quickjs-delay");
        std::fs::write(&path, "function delay() { return 42 }\n").expect("write temporary script");
        let mut engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        assert_eq!(engine.delay(), 42);
    }

    #[test]
    fn response_delivers_status_headers_and_body() {
        use std::sync::Arc;

        use wrkrs_engine::ScriptEngine;
        use wrkrs_engine_tests::fixtures::FakeThread;

        let path = std::env::temp_dir().join("wrkrs-quickjs-response");
        std::fs::write(
            &path,
            "var seen\nfunction response(status, headers, body) { seen = [status, headers[\"Content-Type\"], body] }\n",
        )
        .expect("write temporary script");
        let mut engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        engine
            .response(
                201,
                &[("Content-Type".to_owned(), "text/plain".to_owned())],
                b"payload",
            )
            .unwrap();
        assert_eq!(
            engine.get_global("seen").unwrap(),
            Value::Table(vec![
                (Value::Str("0".to_owned()), Value::Int(201)),
                (
                    Value::Str("1".to_owned()),
                    Value::Str("text/plain".to_owned()),
                ),
                (Value::Str("2".to_owned()), Value::Str("payload".to_owned()),),
            ])
        );
    }

    #[test]
    fn done_receives_summary_and_stats() {
        use std::sync::Arc;

        use wrkrs_engine::{ErrorCounts, ScriptEngine, Summary};
        use wrkrs_engine_tests::fixtures::{FakeStats, FakeThread};

        let path = std::env::temp_dir().join("wrkrs-quickjs-done");
        std::fs::write(
            &path,
            "var seen\nfunction done(summary, latency, requests) {\n\
             seen = [summary.duration, summary.errors.connect,\n\
             latency.percentile(99.0), requests.length]\n\
             }\n",
        )
        .expect("write temporary script");
        let mut engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
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
            .unwrap();
        assert_eq!(
            engine.get_global("seen").unwrap(),
            Value::Table(vec![
                (Value::Str("0".to_owned()), Value::Int(5_000_000)),
                (Value::Str("1".to_owned()), Value::Int(2)),
                (Value::Str("2".to_owned()), Value::Int(100)),
                (Value::Str("3".to_owned()), Value::Int(7)),
            ])
        );
    }

    #[test]
    fn capabilities_reflect_the_loaded_script() {
        use std::sync::Arc;

        use wrkrs_engine::ScriptEngine;
        use wrkrs_engine_tests::fixtures::FakeThread;

        let mut engine = QuickJSEngine::new(&spec(None)).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        let capabilities = engine.capabilities();
        assert!(capabilities.is_static);
        assert!(!capabilities.wants_response);

        let path = std::env::temp_dir().join("wrkrs-quickjs-full");
        std::fs::write(
            &path,
            "function request() { return \"GET / HTTP/1.1\\r\\n\\r\\n\" }\n\
             function response(status, headers, body) {}\n\
             function delay() { return 0 }\n\
             function done(summary, latency, requests) {}\n",
        )
        .expect("write temporary script");
        let mut engine = QuickJSEngine::new(&spec(Some(&path))).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        let capabilities = engine.capabilities();
        assert!(!capabilities.is_static);
        assert!(capabilities.wants_response);
        assert!(capabilities.has_delay);
        assert!(capabilities.has_done);
    }
}
