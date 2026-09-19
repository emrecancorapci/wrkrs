use std::path::Path;
use std::sync::{Arc, Mutex};

use mlua::{Lua, Table};
use wrkrs_engine::{EngineError, ResolveApi, ScriptSpec};

/// The wrk default environment, embedded verbatim from the C source so
/// behavior stays identical, table iteration order included.
const WRK_LUA: &str = include_str!("../../../../../src/wrk.lua");

/// Shared slot holding the host resolver the lookup functions use.
type ResolverSlot = Arc<Mutex<Option<Arc<dyn ResolveApi>>>>;

// Unused by the library until the userdata and callback commits land,
// the allow comes off with them.
#[allow(dead_code)]
mod value;

/// One Lua scripting environment driven by the host.
pub struct LuaEngine {
    // Read only by tests until the trait implementation lands, the
    // allow comes off with that commit.
    #[allow(dead_code)]
    lua: Lua,
    #[allow(dead_code)]
    resolver: ResolverSlot,
}

impl LuaEngine {
    /// Builds one environment from a spec.
    ///
    /// Installs the wrk table with the URL parts and headers, then runs
    /// the script file when the spec carries one.
    pub fn new(spec: &ScriptSpec) -> Result<Self, EngineError> {
        let lua = new_state()?;
        install_wrk_table(&lua, spec)?;
        run_script_file(&lua, spec.script.as_deref());
        Ok(LuaEngine {
            lua,
            resolver: Arc::new(Mutex::new(None)),
        })
    }
}

/// The message the scripting VM reported for an error.
pub(crate) fn vm_message(error: &mlua::Error) -> String {
    match error {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(message) => message.clone(),
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

#[cfg(test)]
mod tests {
    use super::LuaEngine;
    use wrkrs_engine::ScriptSpec;
    use wrkrs_engine::UrlRef;

    fn spec(script: Option<&std::path::Path>) -> ScriptSpec {
        ScriptSpec {
            url: "http://example.test:8080/some/path".to_owned(),
            parts: UrlRef {
                scheme: Some("http".to_owned()),
                host: Some("example.test".to_owned()),
                port: Some("8080".to_owned()),
                path: "/some/path".to_owned(),
            },
            script: script.map(std::path::Path::to_path_buf),
            headers: vec![],
        }
    }

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
}
