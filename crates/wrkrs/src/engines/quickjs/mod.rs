//! The QuickJS scripting engine.
//!
//! Ports the wrk scripting surface to JavaScript. The wrk object and
//! the callbacks follow the Lua engine semantics, with the differences
//! documented in ENGINES.md.

use rquickjs::{Context, IntoJs, Runtime};
use wrkrs_engine::{EngineError, ScriptSpec};

/// One QuickJS scripting environment driven by the host.
pub struct QuickJSEngine {
    #[allow(dead_code)]
    runtime: Runtime,
    // The callbacks read this, the allow comes off with that commit.
    #[allow(dead_code)]
    context: Context,
}

impl QuickJSEngine {
    /// Builds one environment from a spec.
    pub fn new(spec: &ScriptSpec) -> Result<Self, EngineError> {
        let runtime = Runtime::new().map_err(|error| EngineError::Runtime(error.to_string()))?;
        let context =
            Context::full(&runtime).map_err(|error| EngineError::Runtime(error.to_string()))?;
        let engine = QuickJSEngine { runtime, context };
        engine.with(|ctx| install_wrk(ctx, spec))?;
        Ok(engine)
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
// The error mapping uses this, the allow comes off with that commit.
#[allow(dead_code)]
fn exception_message(ctx: &rquickjs::Ctx<'_>) -> String {
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
    ctx.globals().set("wrk", wrk)?;
    Ok(())
}

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
}
