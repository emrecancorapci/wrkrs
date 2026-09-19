use std::net::SocketAddr;
use std::path::Path;
use std::process;
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Function, Lua, Table, Value as LuaValue};
use wrkrs_engine::{
    Capabilities, EngineError, ResolveApi, ScriptEngine, ScriptSpec, StatsView, Summary, ThreadApi,
    Value,
};

use self::stats::StatsHandle;
use self::thread::{Address, ThreadHandle};
use self::value::{lua_to_value, value_to_lua};

/// The wrk default environment, embedded verbatim from the C source so
/// behavior stays identical, table iteration order included.
const WRK_LUA: &str = include_str!("../../../../../src/wrk.lua");

/// Shared slot holding the host resolver the lookup functions use.
type ResolverSlot = Arc<Mutex<Option<Arc<dyn ResolveApi>>>>;

mod stats;
mod thread;
mod value;

/// One Lua scripting environment driven by the host.
pub struct LuaEngine {
    lua: Lua,
    resolver: ResolverSlot,
}

impl LuaEngine {
    /// Builds one environment from a spec.
    ///
    /// Installs the wrk table with the URL parts and headers, wires the
    /// lookup functions, then runs the script file when the spec carries
    /// one.
    pub fn new(spec: &ScriptSpec) -> Result<Self, EngineError> {
        let lua = new_state()?;
        let resolver: ResolverSlot = Arc::new(Mutex::new(None));
        install_wrk_table(&lua, spec)?;
        install_lookup(&lua, &resolver)?;
        run_script_file(&lua, spec.script.as_deref());
        Ok(LuaEngine { lua, resolver })
    }

    /// The wrk table of this environment.
    fn wrk_table(&self) -> Result<Table, EngineError> {
        self.lua.globals().get::<Table>("wrk").map_err(runtime)
    }

    /// Whether a global holds a function, the script.c probe.
    fn is_global_function(&self, name: &str) -> bool {
        self.lua.globals().get::<Function>(name).is_ok()
    }
}

/// Registry factory that builds a [`LuaEngine`].
pub fn factory(spec: &ScriptSpec) -> Result<Box<dyn ScriptEngine>, EngineError> {
    Ok(Box::new(LuaEngine::new(spec)?))
}

/// Maps a VM error to an engine runtime error.
fn runtime(error: mlua::Error) -> EngineError {
    EngineError::Runtime(vm_message(&error))
}

/// The message the scripting VM reported for an error.
pub(crate) fn vm_message(error: &mlua::Error) -> String {
    match error {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(message) => message.clone(),
        mlua::Error::CallbackError { cause, .. } => vm_message(cause),
        other => other.to_string(),
    }
}

/// Creates a Lua state with every standard library.
///
/// wrk calls luaL_openlibs so scripts can write through io (see
/// scripts/report.lua) and the debug library is part of that set.
fn new_state() -> Result<Lua, EngineError> {
    // SAFETY: loading all standard libraries matches the C tool, where
    // luaL_openlibs exposes io, os, and debug to scripts. Scripts come
    // from the user who runs the benchmark, the same trust model as C.
    Ok(unsafe { Lua::unsafe_new() })
}

/// Runs the embedded wrk environment and fills the wrk table.
fn install_wrk_table(lua: &Lua, spec: &ScriptSpec) -> Result<(), EngineError> {
    let wrk: Table = lua
        .load(WRK_LUA)
        .eval()
        .map_err(|error| EngineError::Runtime(vm_message(&error)))?;
    lua.globals()
        .set("wrk", wrk.clone())
        .map_err(|error| EngineError::Runtime(vm_message(&error)))?;

    // The URL parts overwrite the wrk.lua defaults the way script.c
    // does: absent parts become nil, path always carries a value.
    wrk.set("scheme", spec.parts.scheme.clone())
        .map_err(|error| EngineError::Runtime(vm_message(&error)))?;
    wrk.set("host", spec.parts.host.clone())
        .map_err(|error| EngineError::Runtime(vm_message(&error)))?;
    wrk.set("port", spec.parts.port.clone())
        .map_err(|error| EngineError::Runtime(vm_message(&error)))?;
    wrk.set("path", spec.parts.path.clone())
        .map_err(|error| EngineError::Runtime(vm_message(&error)))?;

    let headers: Table = wrk
        .get("headers")
        .map_err(|error| EngineError::Runtime(vm_message(&error)))?;
    for (name, value) in &spec.headers {
        headers
            .raw_set(name.clone(), value.clone())
            .map_err(|error| EngineError::Runtime(vm_message(&error)))?;
    }
    Ok(())
}

/// Runs the script file for an environment.
///
/// wrk parity for load failures: print `path: message` on stderr and
/// keep the environment running with the default behavior. script.c
/// reports the error and does not exit.
fn run_script_file(lua: &Lua, script: Option<&Path>) {
    let Some(path) = script else {
        return;
    };
    let outcome = match std::fs::read_to_string(path) {
        Ok(source) => lua.load(source).set_name(path.display().to_string()).exec(),
        Err(error) => Err(mlua::Error::RuntimeError(format!(
            "cannot open {}: {error}",
            path.display()
        ))),
    };
    if let Err(error) = &outcome {
        eprintln!("{}: {}", path.display(), vm_message(error));
    }
}

/// Installs wrk.lookup and wrk.connect into the wrk table.
///
/// The functions read the shared resolver slot, which the host fills
/// when resolve runs. A call before that fails the way a missing host
/// service would.
fn install_lookup(lua: &Lua, slot: &ResolverSlot) -> Result<(), EngineError> {
    let wrk: Table = lua.globals().get::<Table>("wrk").map_err(runtime)?;

    let lookup_slot = slot.clone();
    let lookup = lua
        .create_function(move |lua, (host, service): (String, String)| {
            let resolver = shared_resolver(&lookup_slot)?;
            let addresses = resolver.lookup(&host, &service).map_err(|error| {
                // wrk prints unable to resolve and exits, the host prints
                // this message and exits the same way.
                mlua::Error::RuntimeError(format!("unable to resolve {host}:{service} {error}"))
            })?;
            let table = lua
                .create_table()
                .map_err(|error| mlua::Error::RuntimeError(vm_message(&error)))?;
            for (index, address) in addresses.into_iter().enumerate() {
                let userdata = lua
                    .create_userdata(Address(address))
                    .map_err(|error| mlua::Error::RuntimeError(vm_message(&error)))?;
                table
                    .raw_set(index + 1, userdata)
                    .map_err(|error| mlua::Error::RuntimeError(vm_message(&error)))?;
            }
            Ok(table)
        })
        .map_err(runtime)?;
    wrk.set("lookup", lookup).map_err(runtime)?;

    let connect_slot = slot.clone();
    let connect = lua
        .create_function(move |_, address: AnyUserData| {
            let resolver = shared_resolver(&connect_slot)?;
            let address = address
                .borrow::<Address>()
                .map_err(|error| mlua::Error::RuntimeError(vm_message(&error)))?;
            Ok(resolver.connect(&address.0))
        })
        .map_err(runtime)?;
    wrk.set("connect", connect).map_err(runtime)?;
    Ok(())
}

/// Takes the current resolver for a lookup function call.
fn shared_resolver(slot: &ResolverSlot) -> Result<Arc<dyn ResolveApi>, mlua::Error> {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or_else(|| {
            mlua::Error::RuntimeError(
                "wrk.lookup is not available before the run resolves".to_owned(),
            )
        })
}

impl ScriptEngine for LuaEngine {
    fn create(spec: &ScriptSpec) -> Result<Self, EngineError>
    where
        Self: Sized,
    {
        LuaEngine::new(spec)
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
        let wrk = self.wrk_table()?;
        let resolve: Function = wrk.get("resolve").map_err(runtime)?;
        resolve
            .call::<()>((host.to_owned(), service.to_owned()))
            .map_err(|error| EngineError::Resolve(vm_message(&error)))?;

        let addrs: Table = wrk.get("addrs").map_err(runtime)?;
        let mut addresses = Vec::new();
        for entry in addrs.sequence_values::<AnyUserData>() {
            let userdata = entry.map_err(runtime)?;
            let address = userdata.borrow::<Address>().map_err(runtime)?;
            addresses.push(address.0);
        }
        Ok(addresses)
    }

    fn setup(&mut self, thread: Arc<dyn ThreadApi>) -> Result<(), EngineError> {
        let userdata = self
            .lua
            .create_userdata(ThreadHandle(thread))
            .map_err(runtime)?;
        let wrk = self.wrk_table()?;
        let setup: Function = wrk.get("setup").map_err(runtime)?;
        setup.call::<()>(userdata).map_err(runtime)
    }

    fn init(&mut self, thread: Arc<dyn ThreadApi>, args: &[String]) -> Result<(), EngineError> {
        let userdata = self
            .lua
            .create_userdata(ThreadHandle(thread))
            .map_err(runtime)?;
        let wrk = self.wrk_table()?;
        wrk.set("thread", userdata).map_err(runtime)?;

        let table = self.lua.create_table().map_err(runtime)?;
        // wrk fills the args table from index zero, one quirk of
        // script_init that scripts observe.
        for (index, arg) in args.iter().enumerate() {
            table.raw_set(index, arg.clone()).map_err(runtime)?;
        }
        let init: Function = wrk.get("init").map_err(runtime)?;
        init.call::<()>(table).map_err(runtime)
    }

    fn delay(&mut self) -> u64 {
        let result: Result<f64, _> = self
            .lua
            .globals()
            .get::<Function>("delay")
            .and_then(|function| function.call(()));
        match result {
            Ok(delay) => delay as u64,
            Err(error) => {
                // wrk calls delay unprotected, so a script error aborts
                // the whole run.
                eprintln!("{}", vm_message(&error));
                process::exit(1);
            }
        }
    }

    fn request(&mut self) -> Result<Vec<u8>, EngineError> {
        let function = match self.lua.globals().get::<Function>("request") {
            Ok(function) => function,
            Err(_) => self.wrk_table()?.get("request").map_err(runtime)?,
        };
        let value: LuaValue = function.call(()).map_err(runtime)?;
        request_bytes(&value).ok_or_else(|| {
            EngineError::Runtime("request must return a string or a number".to_owned())
        })
    }

    fn response(
        &mut self,
        status: u16,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(), EngineError> {
        let table = self.lua.create_table().map_err(runtime)?;
        for (name, value) in headers {
            table
                .raw_set(name.clone(), value.clone())
                .map_err(runtime)?;
        }
        let body = self.lua.create_string(body).map_err(runtime)?;
        let function: Function = self.lua.globals().get("response").map_err(runtime)?;
        function.call::<()>((status, table, body)).map_err(runtime)
    }

    fn done(
        &mut self,
        summary: &Summary,
        latency: Arc<dyn StatsView>,
        requests: Arc<dyn StatsView>,
    ) -> Result<(), EngineError> {
        let errors = self.lua.create_table().map_err(runtime)?;
        errors
            .raw_set("connect", summary.errors.connect)
            .map_err(runtime)?;
        errors
            .raw_set("read", summary.errors.read)
            .map_err(runtime)?;
        errors
            .raw_set("write", summary.errors.write)
            .map_err(runtime)?;
        errors
            .raw_set("status", summary.errors.status)
            .map_err(runtime)?;
        errors
            .raw_set("timeout", summary.errors.timeout)
            .map_err(runtime)?;

        let summary_table = self.lua.create_table().map_err(runtime)?;
        summary_table
            .raw_set("duration", summary.duration)
            .map_err(runtime)?;
        summary_table
            .raw_set("requests", summary.requests)
            .map_err(runtime)?;
        summary_table
            .raw_set("bytes", summary.bytes)
            .map_err(runtime)?;
        summary_table.raw_set("errors", errors).map_err(runtime)?;

        let latency = self
            .lua
            .create_userdata(StatsHandle(latency))
            .map_err(runtime)?;
        let requests = self
            .lua
            .create_userdata(StatsHandle(requests))
            .map_err(runtime)?;
        let function: Function = self.lua.globals().get("done").map_err(runtime)?;
        function
            .call::<()>((summary_table, latency, requests))
            .map_err(runtime)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            is_static: !self.is_global_function("request"),
            wants_response: self.is_global_function("response"),
            has_delay: self.is_global_function("delay"),
            has_done: self.is_global_function("done"),
        }
    }

    fn get_global(&self, name: &str) -> Result<Value, EngineError> {
        let value: LuaValue = self.lua.globals().get(name).map_err(runtime)?;
        lua_to_value(value)
    }

    fn set_global(&mut self, name: &str, value: &Value) -> Result<(), EngineError> {
        let converted = value_to_lua(&self.lua, value)?;
        self.lua.globals().set(name, converted).map_err(runtime)
    }
}

/// Converts a request callback return value into bytes.
///
/// wrk coerces numbers through lua_tolstring, everything else must be a
/// string already.
fn request_bytes(value: &LuaValue) -> Option<Vec<u8>> {
    match value {
        LuaValue::String(string) => Some(string.as_bytes().to_vec()),
        LuaValue::Integer(integer) => Some(integer.to_string().into_bytes()),
        LuaValue::Number(number) => Some(number.to_string().into_bytes()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::LuaEngine;
    use crate::engines::test_support::spec;

    fn temp_script(name: &str, source: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("wrkrs-lua-{name}"));
        // A failure here means the test environment cannot write its
        // temporary directory, which is outside what the test controls.
        std::fs::write(&path, source).expect("write temporary script");
        path
    }

    fn wrk_table(engine: &LuaEngine) -> mlua::Table {
        engine
            .lua
            .globals()
            .get::<mlua::Table>("wrk")
            .expect("wrk table exists")
    }

    #[test]
    fn installs_the_url_parts_into_the_wrk_table() {
        let engine = LuaEngine::new(&spec(None)).unwrap();
        let wrk = wrk_table(&engine);
        assert_eq!(wrk.get::<String>("scheme").unwrap(), "http");
        assert_eq!(wrk.get::<String>("host").unwrap(), "example.test");
        assert_eq!(wrk.get::<String>("port").unwrap(), "8080");
        assert_eq!(wrk.get::<String>("path").unwrap(), "/some/path");
        assert_eq!(
            wrk.get::<Option<String>>("port").unwrap(),
            Some("8080".into())
        );
    }

    #[test]
    fn keeps_the_default_method_and_empty_headers() {
        let engine = LuaEngine::new(&spec(None)).unwrap();
        let wrk = wrk_table(&engine);
        assert_eq!(wrk.get::<String>("method").unwrap(), "GET");
        let headers = wrk.get::<mlua::Table>("headers").unwrap();
        assert_eq!(headers.raw_len(), 0);
    }

    #[test]
    fn applies_command_line_headers() {
        let mut given = spec(None);
        given.headers = vec![
            ("Accept".to_owned(), "application/json".to_owned()),
            ("X-Custom".to_owned(), "1".to_owned()),
        ];
        let engine = LuaEngine::new(&given).unwrap();
        let headers = wrk_table(&engine).get::<mlua::Table>("headers").unwrap();
        assert_eq!(headers.get::<String>("Accept").unwrap(), "application/json");
        assert_eq!(headers.get::<String>("X-Custom").unwrap(), "1");
    }

    #[test]
    fn script_files_run_and_can_change_the_wrk_table() {
        let script = temp_script("method", "wrk.method = \"POST\"\n");
        let engine = LuaEngine::new(&spec(Some(&script))).unwrap();
        assert_eq!(wrk_table(&engine).get::<String>("method").unwrap(), "POST");
    }

    #[test]
    fn script_load_errors_report_and_continue() {
        let script = temp_script("broken", "this is not lua\n");
        let engine = LuaEngine::new(&spec(Some(&script))).unwrap();
        assert_eq!(wrk_table(&engine).get::<String>("method").unwrap(), "GET");
    }

    #[test]
    fn every_example_script_loads() {
        use wrkrs_engine::ScriptEngine;

        let scripts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts");
        let mut loaded = 0;
        let entries =
            // A read failure means the test tree is not in place, which
            // the test cannot work around.
            std::fs::read_dir(scripts).expect("read the scripts directory");
        for entry in entries {
            let path = entry.expect("read a directory entry").path();
            if path.extension().is_some_and(|extension| extension == "lua") {
                let engine = LuaEngine::new(&spec(Some(&path)))
                    .unwrap_or_else(|error| panic!("{} failed to load: {error}", path.display()));
                let _ = engine.capabilities();
                loaded += 1;
            }
        }
        assert!(loaded >= 9, "expected the example scripts, found {loaded}");
    }

    #[test]
    fn wrk_format_builds_requests_before_init() {
        // Before init the Host header is unset, so wrk.format emits a
        // request without one. The wrk table header assignment of nil
        // leaves the table unchanged.
        let engine = LuaEngine::new(&spec(None)).unwrap();
        let formatted: String = wrk_table(&engine)
            .get::<mlua::Function>("format")
            .unwrap()
            .call(())
            .unwrap();
        assert_eq!(formatted, "GET /some/path HTTP/1.1\r\n\r\n");
    }

    #[test]
    fn resolve_filters_unreachable_addresses() {
        use std::sync::Arc;

        use crate::engines::test_support::{FakeResolver, address};
        use wrkrs_engine::ScriptEngine;

        let mut engine = LuaEngine::new(&spec(None)).unwrap();
        let resolved = engine
            .resolve("example.test", "8080", Arc::new(FakeResolver::default()))
            .unwrap();
        assert_eq!(resolved, vec![address(1)]);
        let addrs = wrk_table(&engine).get::<mlua::Table>("addrs").unwrap();
        assert_eq!(addrs.raw_len(), 1);
    }

    #[test]
    fn resolve_reports_lookup_failures() {
        use std::sync::Arc;

        use crate::engines::test_support::FailingResolver;
        use wrkrs_engine::ScriptEngine;

        let mut engine = LuaEngine::new(&spec(None)).unwrap();
        let error = engine
            .resolve("example.test", "8080", Arc::new(FailingResolver))
            .unwrap_err();
        assert_eq!(
            error.raw_message(),
            "unable to resolve example.test:8080 name or service not known"
        );
    }

    #[test]
    fn setup_assigns_the_first_address_to_the_thread() {
        use std::sync::Arc;

        use crate::engines::test_support::{FakeResolver, FakeThread, address};
        use wrkrs_engine::{ScriptEngine, ThreadApi};

        let mut engine = LuaEngine::new(&spec(None)).unwrap();
        engine
            .resolve("example.test", "8080", Arc::new(FakeResolver::default()))
            .unwrap();
        let thread = Arc::new(FakeThread::default());
        engine.setup(thread.clone()).unwrap();
        assert_eq!(thread.addr(), Some(address(1)));
    }

    #[test]
    fn setup_calls_the_script_setup_function() {
        use std::sync::Arc;

        use crate::engines::test_support::{FakeResolver, FakeThread};
        use wrkrs_engine::{ScriptEngine, ThreadApi, Value};

        let script = temp_script(
            "setup",
            "function setup(thread) thread:set(\"id\", 7) end\n",
        );
        let mut engine = LuaEngine::new(&spec(Some(&script))).unwrap();
        engine
            .resolve("example.test", "8080", Arc::new(FakeResolver::default()))
            .unwrap();
        let thread = Arc::new(FakeThread::default());
        engine.setup(thread.clone()).unwrap();
        assert_eq!(thread.get_global("id").unwrap(), Value::Int(7));
    }

    #[test]
    fn init_passes_args_from_index_zero() {
        use std::sync::Arc;

        use crate::engines::test_support::FakeThread;
        use wrkrs_engine::ScriptEngine;

        let script = temp_script("args", "function init(args) first = args[0] end\n");
        let mut engine = LuaEngine::new(&spec(Some(&script))).unwrap();
        let args = vec!["hello".to_owned(), "world".to_owned()];
        engine.init(Arc::new(FakeThread::default()), &args).unwrap();
        assert_eq!(
            engine.get_global("first").unwrap(),
            wrkrs_engine::Value::Str("hello".to_owned())
        );
    }

    #[test]
    fn default_request_matches_the_core_formatter() {
        use std::sync::Arc;

        use crate::engines::test_support::FakeThread;
        use wrkrs_engine::{ScriptEngine, format_request, host_header};

        let mut engine = LuaEngine::new(&spec(None)).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        let request = engine.request().unwrap();
        let host = host_header("example.test", Some("8080"));
        let expected = format_request("GET", "/some/path", &[], None, Some(&host));
        assert_eq!(request, expected);
    }

    #[test]
    fn delay_returns_the_script_value() {
        use std::sync::Arc;

        use crate::engines::test_support::FakeThread;
        use wrkrs_engine::ScriptEngine;

        let script = temp_script("delay", "function delay() return 42 end\n");
        let mut engine = LuaEngine::new(&spec(Some(&script))).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        assert_eq!(engine.delay(), 42);
    }

    #[test]
    fn response_delivers_status_headers_and_body() {
        use std::sync::Arc;

        use crate::engines::test_support::FakeThread;
        use wrkrs_engine::ScriptEngine;

        let script = temp_script(
            "response",
            "function response(status, headers, body)\n\
             seen_status = status\n\
             seen_type = headers[\"Content-Type\"]\n\
             seen_body = body\n\
             end\n",
        );
        let mut engine = LuaEngine::new(&spec(Some(&script))).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        engine
            .response(
                201,
                &[
                    ("Content-Type".to_owned(), "text/plain".to_owned()),
                    ("Set-Cookie".to_owned(), "a=1".to_owned()),
                    ("Set-Cookie".to_owned(), "b=2".to_owned()),
                ],
                b"payload",
            )
            .unwrap();
        assert_eq!(
            engine.get_global("seen_status").unwrap(),
            wrkrs_engine::Value::Int(201)
        );
        assert_eq!(
            engine.get_global("seen_type").unwrap(),
            wrkrs_engine::Value::Str("text/plain".to_owned())
        );
        assert_eq!(
            engine.get_global("seen_body").unwrap(),
            wrkrs_engine::Value::Str("payload".to_owned())
        );
    }

    #[test]
    fn done_receives_summary_and_stats() {
        use std::sync::Arc;

        use crate::engines::test_support::{FakeStats, FakeThread};
        use wrkrs_engine::{ErrorCounts, ScriptEngine, Summary};

        let script = temp_script(
            "done",
            "function done(summary, latency, requests)\n\
             seen_duration = summary.duration\n\
             seen_connect = summary.errors.connect\n\
             seen_percentile = latency:percentile(99.0)\n\
             seen_popcount = #requests\n\
             end\n",
        );
        let mut engine = LuaEngine::new(&spec(Some(&script))).unwrap();
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
            engine.get_global("seen_duration").unwrap(),
            wrkrs_engine::Value::Int(5_000_000)
        );
        assert_eq!(
            engine.get_global("seen_connect").unwrap(),
            wrkrs_engine::Value::Int(2)
        );
        assert_eq!(
            engine.get_global("seen_percentile").unwrap(),
            wrkrs_engine::Value::Int(100)
        );
        assert_eq!(
            engine.get_global("seen_popcount").unwrap(),
            wrkrs_engine::Value::Int(7)
        );
    }

    #[test]
    fn capabilities_reflect_the_loaded_script() {
        use std::sync::Arc;

        use crate::engines::test_support::FakeThread;
        use wrkrs_engine::ScriptEngine;

        let mut engine = LuaEngine::new(&spec(None)).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        let capabilities = engine.capabilities();
        assert!(capabilities.is_static);
        assert!(!capabilities.wants_response);
        assert!(!capabilities.has_delay);
        assert!(!capabilities.has_done);

        let script = temp_script(
            "full",
            "function request() return \"GET / HTTP/1.1\\r\\n\\r\\n\" end\n\
             function response(status, headers, body) end\n\
             function delay() return 0 end\n\
             function done(summary, latency, requests) end\n",
        );
        let mut engine = LuaEngine::new(&spec(Some(&script))).unwrap();
        engine.init(Arc::new(FakeThread::default()), &[]).unwrap();
        let capabilities = engine.capabilities();
        assert!(!capabilities.is_static);
        assert!(capabilities.wants_response);
        assert!(capabilities.has_delay);
        assert!(capabilities.has_done);
    }
}
