//! The address object scripts get from wrk.lookup.

use std::net::SocketAddr;

use rquickjs::class::{Trace, Tracer};
use rquickjs::{Class, JsLifetime};

/// An address, printable as host and service.
#[rquickjs::class(rename_all = "camelCase")]
pub struct AddressObject {
    /// The wrapped host address.
    pub address: SocketAddr,
}

impl<'js> Trace<'js> for AddressObject {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

unsafe impl<'js> JsLifetime<'js> for AddressObject {
    type Changed<'to> = Self;
}

#[rquickjs::methods]
impl AddressObject {
    #[qjs(rename = "toString")]
    fn as_text(&self) -> String {
        self.address.to_string()
    }
}

/// Creates a new address instance for a context.
pub fn instance<'js>(
    ctx: rquickjs::Ctx<'js>,
    address: SocketAddr,
) -> Result<rquickjs::Class<'js, AddressObject>, rquickjs::Error> {
    Class::<AddressObject>::instance(ctx, AddressObject { address })
}

#[cfg(test)]
mod tests {
    use rquickjs::{Context, Runtime};

    use super::instance;

    #[test]
    fn addresses_print_as_host_and_service() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        context
            .with(|ctx| {
                rquickjs::Class::<super::AddressObject>::define(&ctx.globals())?;
                let address = instance(ctx.clone(), "127.0.0.1:8080".parse().unwrap())?;
                ctx.globals().set("addr", address)
            })
            .unwrap();
        let printed: String = context.with(|ctx| ctx.eval("String(addr)")).unwrap();
        assert_eq!(printed, "127.0.0.1:8080");
    }
}
