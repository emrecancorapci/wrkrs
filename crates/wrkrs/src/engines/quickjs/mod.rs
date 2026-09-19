//! The QuickJS scripting engine.
//!
//! Ports the wrk scripting surface to JavaScript. The wrk object and
//! the callbacks follow the Lua engine semantics, with the differences
//! documented in ENGINES.md.

use rquickjs::{Context, Runtime};
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
    pub fn new(_spec: &ScriptSpec) -> Result<Self, EngineError> {
        let runtime = Runtime::new().map_err(|error| EngineError::Runtime(error.to_string()))?;
        let context =
            Context::full(&runtime).map_err(|error| EngineError::Runtime(error.to_string()))?;
        Ok(QuickJSEngine { runtime, context })
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
}
