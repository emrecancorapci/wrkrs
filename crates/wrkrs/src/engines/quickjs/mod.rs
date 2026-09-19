//! The QuickJS scripting engine.
//!
//! Ports the wrk scripting surface to JavaScript. The wrk object and
//! the callbacks follow the Lua engine semantics, with the differences
//! documented in ENGINES.md.

use std::sync::{Arc, Mutex};

use rquickjs::{Context, IntoJs, Runtime};
use wrkrs_engine::{EngineError, ResolveApi, ScriptSpec};

use self::address::AddressObject;

/// Shared slot holding the host resolver the lookup functions use.
type ResolverSlot = Arc<Mutex<Option<Arc<dyn ResolveApi>>>>;

/// One QuickJS scripting environment driven by the host.
pub struct QuickJSEngine {
    #[allow(dead_code)]
    runtime: Runtime,
    // The callbacks read this, the allow comes off with that commit.
    #[allow(dead_code)]
    context: Context,
    // The callbacks read this, the allow comes off with that commit.
    #[allow(dead_code)]
    resolver: ResolverSlot,
}

impl QuickJSEngine {
    /// Builds one environment from a spec.
    pub fn new(spec: &ScriptSpec) -> Result<Self, EngineError> {
        let runtime = Runtime::new().map_err(|error| EngineError::Runtime(error.to_string()))?;
        let context =
            Context::full(&runtime).map_err(|error| EngineError::Runtime(error.to_string()))?;
        let engine = QuickJSEngine {
            runtime,
            context,
            resolver: Arc::new(Mutex::new(None)),
        };
        engine.with(|ctx| install_wrk(ctx, spec))?;
        engine.with(|ctx| {
            thread::define(ctx)?;
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
    // The callbacks use this, the allow comes off with that commit.
    #[allow(dead_code)]
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
// The callbacks use this, the allow comes off with that commit.
#[allow(dead_code)]
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
    Ok(())
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

// Unused until the callbacks land, the allow comes off with them.
#[allow(dead_code)]
mod address;
#[allow(dead_code)]
mod stats;
#[allow(dead_code)]
mod thread;
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::QuickJSEngine;
    use crate::engines::test_support::spec;

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
}
