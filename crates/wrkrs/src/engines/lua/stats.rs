//! The stats userdata scripts see in done.
//!
//! Ports the wrk.stats metatable from script.c: min, max, mean, and
//! stdev fields, the percentile method, the call operator returning
//! value and count, and the length operator returning the popcount.

use std::sync::Arc;

use mlua::{IntoLua, UserData, UserDataMethods, Value as LuaValue};
use wrkrs_engine::StatsView;

/// The stats userdata delegating to one host statistics view.
pub struct StatsHandle(pub Arc<dyn StatsView>);

/// Converts a statistic into the Lua integer wrk pushes.
fn lua_integer(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

impl UserData for StatsHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method("__index", |lua, stats, key: String| match key.as_str() {
            "min" => Ok(lua_integer(stats.0.min()).into_lua(lua)?),
            "max" => Ok(lua_integer(stats.0.max()).into_lua(lua)?),
            "mean" => Ok(stats.0.mean().into_lua(lua)?),
            "stdev" => Ok(stats.0.stdev().into_lua(lua)?),
            "percentile" => {
                let view = stats.0.clone();
                let function =
                    lua.create_function(move |_, (_, percentile): (LuaValue, f64)| {
                        Ok(lua_integer(view.percentile(percentile)))
                    })?;
                Ok(function.into_lua(lua)?)
            }
            _ => Ok(LuaValue::Nil),
        });
        methods.add_meta_method("__call", |_, stats, index: i64| {
            // wrk subtracts one from the script supplied index because
            // stats_value_at walks occupied slots from zero.
            if index < 1 {
                return Err(mlua::Error::RuntimeError(
                    "stats index must be at least 1".to_owned(),
                ));
            }
            let (value, count) = stats.0.value_at((index - 1) as u64);
            Ok((lua_integer(value), lua_integer(count)))
        });
        methods.add_meta_method("__len", |_, stats, ()| Ok(lua_integer(stats.0.popcount())));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_support::FakeStats;
    use super::StatsHandle;

    fn stats_global(lua: &mlua::Lua) {
        let userdata = lua
            .create_userdata(StatsHandle(Arc::new(FakeStats)))
            .unwrap();
        lua.globals().set("latency", userdata).unwrap();
    }

    #[test]
    fn exposes_the_summary_fields() {
        let lua = mlua::Lua::new();
        stats_global(&lua);
        let min: i64 = lua.load("return latency.min").eval().unwrap();
        let max: i64 = lua.load("return latency.max").eval().unwrap();
        let mean: f64 = lua.load("return latency.mean").eval().unwrap();
        let stdev: f64 = lua.load("return latency.stdev").eval().unwrap();
        assert_eq!((min, max, mean, stdev), (100, 900, 250.5, 12.25));
    }

    #[test]
    fn percentile_takes_the_script_value() {
        let lua = mlua::Lua::new();
        stats_global(&lua);
        let percentile: i64 = lua.load("return latency:percentile(99.0)").eval().unwrap();
        assert_eq!(percentile, 100);
    }

    #[test]
    fn call_returns_value_and_count_for_one_based_slots() {
        let lua = mlua::Lua::new();
        stats_global(&lua);
        let (value, count): (i64, i64) = lua.load("return latency(2)").eval().unwrap();
        assert_eq!((value, count), (101, 10));
    }

    #[test]
    fn length_returns_the_popcount() {
        let lua = mlua::Lua::new();
        stats_global(&lua);
        let length: i64 = lua.load("return #latency").eval().unwrap();
        assert_eq!(length, 7);
    }
}
