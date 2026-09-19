//! The stats object scripts see in done.

use std::sync::Arc;

use rquickjs::class::{Trace, Tracer};
use rquickjs::{Class, JsLifetime};
use wrkrs_engine::StatsView;

/// The stats object delegating to one host statistics view.
#[rquickjs::class(rename_all = "camelCase")]
pub struct StatsObject {
    /// The shared statistics view.
    pub view: Arc<dyn StatsView>,
}

impl<'js> Trace<'js> for StatsObject {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

unsafe impl<'js> JsLifetime<'js> for StatsObject {
    type Changed<'to> = Self;
}

#[rquickjs::methods]
impl StatsObject {
    #[qjs(get)]
    fn min(&self) -> i64 {
        self.view.min() as i64
    }

    #[qjs(get)]
    fn max(&self) -> i64 {
        self.view.max() as i64
    }

    #[qjs(get)]
    fn mean(&self) -> f64 {
        self.view.mean()
    }

    #[qjs(get)]
    fn stdev(&self) -> f64 {
        self.view.stdev()
    }

    #[qjs(get, rename = "length")]
    fn popcount(&self) -> i64 {
        self.view.popcount() as i64
    }

    fn percentile(&self, percentile: f64) -> i64 {
        self.view.percentile(percentile) as i64
    }

    /// Returns value and count for a one based slot, the JavaScript
    /// shape of the Lua call operator.
    fn call<'js>(
        &self,
        ctx: rquickjs::Ctx<'js>,
        index: i64,
    ) -> Result<rquickjs::Array<'js>, rquickjs::Error> {
        if index < 1 {
            return Err(rquickjs::Exception::throw_message(
                &ctx,
                "stats index must be at least 1",
            ));
        }
        let (value, count) = self.view.value_at((index - 1) as u64);
        let pair = rquickjs::Array::new(ctx)?;
        pair.set(0, value as i64)?;
        pair.set(1, count as i64)?;
        Ok(pair)
    }
}

/// Creates a new stats instance for a context.
pub fn instance<'js>(
    ctx: rquickjs::Ctx<'js>,
    view: Arc<dyn StatsView>,
) -> Result<Class<'js, StatsObject>, rquickjs::Error> {
    Class::<StatsObject>::instance(ctx, StatsObject { view })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rquickjs::{Context, Runtime};

    use super::instance;
    use crate::engines::test_support::FakeStats;

    fn stats_global(context: &Context) {
        context
            .with(|ctx| {
                rquickjs::Class::<super::StatsObject>::define(&ctx.globals())?;
                let stats = instance(ctx.clone(), Arc::new(FakeStats))?;
                ctx.globals().set("latency", stats)
            })
            .unwrap();
    }

    #[test]
    fn exposes_the_summary_fields() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        stats_global(&context);
        let values: Vec<f64> = context
            .with(|ctx| {
                ctx.eval("[latency.min, latency.max, latency.mean, latency.stdev, latency.length]")
            })
            .unwrap();
        assert_eq!(values, vec![100.0, 900.0, 250.5, 12.25, 7.0]);
    }

    #[test]
    fn percentile_takes_the_script_value() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        stats_global(&context);
        let percentile: i64 = context
            .with(|ctx| ctx.eval("latency.percentile(99.0)"))
            .unwrap();
        assert_eq!(percentile, 100);
    }

    #[test]
    fn call_returns_value_and_count_for_one_based_slots() {
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        stats_global(&context);
        let pair: Vec<i64> = context.with(|ctx| ctx.eval("latency.call(2)")).unwrap();
        assert_eq!(pair, vec![101, 10]);
    }
}
