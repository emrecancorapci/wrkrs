//! The thread object scripts see in setup and as wrk.thread.

use std::sync::Arc;

use rquickjs::class::{Trace, Tracer};
use rquickjs::{Class, Ctx, JsLifetime, Value};
use wrkrs_engine::{EngineError, ThreadApi};

use super::address::{self, AddressObject};
use super::value::{js_to_value, value_to_js};

/// The thread object backing addr, get, set, and stop.
#[rquickjs::class(rename_all = "camelCase")]
pub struct ThreadObject {
    /// The shared host handle.
    pub api: Arc<dyn ThreadApi>,
}

impl<'js> Trace<'js> for ThreadObject {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

unsafe impl<'js> JsLifetime<'js> for ThreadObject {
    type Changed<'to> = Self;
}

fn host_error<'js>(ctx: &Ctx<'js>, error: EngineError) -> rquickjs::Error {
    rquickjs::Exception::throw_message(ctx, &error.to_string())
}

#[rquickjs::methods]
impl ThreadObject {
    #[qjs(get)]
    fn addr<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>, rquickjs::Error> {
        match self.api.addr() {
            Some(address) => Ok(address::instance(ctx, address)?.into_value()),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(set, rename = "addr")]
    fn set_addr<'js>(
        &self,
        _ctx: Ctx<'js>,
        address: Class<'js, AddressObject>,
    ) -> Result<(), rquickjs::Error> {
        let address = address.try_borrow()?;
        self.api.set_addr(address.address);
        Ok(())
    }

    fn get<'js>(&self, ctx: Ctx<'js>, name: String) -> Result<Value<'js>, rquickjs::Error> {
        let value = self
            .api
            .get_global(&name)
            .map_err(|error| host_error(&ctx, error))?;
        value_to_js(&ctx, &value)
    }

    fn set<'js>(
        &self,
        _ctx: Ctx<'js>,
        name: String,
        value: Value<'js>,
    ) -> Result<(), rquickjs::Error> {
        let value = js_to_value(&value).map_err(|error| host_error(&_ctx, error))?;
        self.api
            .set_global(&name, &value)
            .map_err(|error| host_error(&_ctx, error))
    }

    fn stop(&self) {
        self.api.stop();
    }
}

/// Creates a new thread instance for a context.
pub fn instance<'js>(
    ctx: Ctx<'js>,
    api: Arc<dyn ThreadApi>,
) -> Result<Class<'js, ThreadObject>, rquickjs::Error> {
    Class::<ThreadObject>::instance(ctx, ThreadObject { api })
}

/// Registers the thread related classes on a context.
pub fn define(ctx: &Ctx<'_>) -> Result<(), rquickjs::Error> {
    Class::<ThreadObject>::define(&ctx.globals())?;
    Class::<AddressObject>::define(&ctx.globals())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rquickjs::{Context, Runtime};

    use super::{define, instance};
    use wrkrs_engine::ThreadApi;
    use wrkrs_engine_tests::fixtures::FakeThread;

    fn thread_global(context: &Context, api: Arc<FakeThread>) {
        context
            .with(|ctx| {
                define(&ctx)?;
                let thread = instance(ctx.clone(), api)?;
                ctx.globals().set("thread", thread)
            })
            .unwrap();
    }

    #[test]
    fn set_and_get_transfer_values() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        let api = Arc::new(FakeThread::default());
        thread_global(&context, api.clone());
        context
            .with(|ctx| ctx.eval::<(), _>("thread.set(\"id\", 3)"))
            .unwrap();
        let id: i64 = context.with(|ctx| ctx.eval("thread.get(\"id\")")).unwrap();
        assert_eq!(id, 3);
    }

    #[test]
    fn set_rejects_functions() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        thread_global(&context, Arc::new(FakeThread::default()));
        context
            .with(|ctx| ctx.eval::<(), _>("thread.set(\"f\", () => 1)"))
            .unwrap_err();
        let message = context.with(|ctx| super::super::exception_message(&ctx));
        assert!(message.contains("cannot transfer 'function' to thread"));
    }

    #[test]
    fn stop_stops_the_thread() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        let api = Arc::new(FakeThread::default());
        thread_global(&context, api.clone());
        context
            .with(|ctx| ctx.eval::<(), _>("thread.stop()"))
            .unwrap();
        assert!(api.stopped());
    }

    #[test]
    fn addr_round_trips_through_assignment() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        let api = Arc::new(FakeThread::default());
        thread_global(&context, api.clone());
        context
            .with(|ctx| {
                let address = super::super::address::instance(
                    ctx.clone(),
                    "127.0.0.1:9090".parse().unwrap(),
                )?;
                ctx.globals().set("wrkAddress", address)
            })
            .unwrap();
        context
            .with(|ctx| ctx.eval::<(), _>("thread.addr = wrkAddress"))
            .unwrap();
        assert_eq!(api.addr(), Some("127.0.0.1:9090".parse().unwrap()));
        let printed: String = context.with(|ctx| ctx.eval("String(thread.addr)")).unwrap();
        assert_eq!(printed, "127.0.0.1:9090");
    }
}
