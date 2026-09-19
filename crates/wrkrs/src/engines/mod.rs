use std::path::Path;

use wrkrs_engine::{EngineError, ScriptEngine, ScriptSpec};

#[cfg(any(feature = "engine-luajit", feature = "engine-lua54"))]
pub mod lua;
#[cfg(feature = "engine-quickjs")]
pub mod quickjs;
#[cfg(feature = "engine-stub")]
pub mod stub;

#[cfg(test)]
pub(crate) mod test_support;

/// Engine names this project ships, used to add rebuild hints.
const PROJECT_ENGINES: &[&str] = &["luajit", "lua54", "quickjs", "stub"];

/// Error choosing the engine for a run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectionError {
    /// `-e` was given without a script to run.
    #[error("option -e requires a script (-s)")]
    EngineWithoutScript,
    /// The named engine exists in this project but is not compiled in.
    #[error("engine '{0}' not compiled in, rebuild with --features engine-{0}")]
    KnownEngineNotCompiled(String),
    /// The named engine is unknown to this project.
    #[error("engine '{0}' not compiled in, compiled engines: {1}")]
    UnknownEngine(String, String),
    /// No compiled engine handles the script extension.
    #[error("no engine handles the '{0}' extension, compiled engines: {1}")]
    UnknownExtension(String, String),
    /// The script has no extension to dispatch on.
    #[error("'{0}' has no extension, compiled engines: {1}")]
    NoExtension(String, String),
}

/// One compiled-in scripting engine.
///
/// Entries are registered at compile time from cargo features. The table
/// never changes at runtime.
pub struct EngineEntry {
    /// Engine name accepted by `-e`, for example `stub`.
    pub name: &'static str,
    /// File extensions this engine dispatches on, without the dot.
    pub extensions: &'static [&'static str],
    /// One line description printed by `-E`.
    pub description: &'static str,
    /// Builds one engine instance for a spec.
    pub factory: fn(&ScriptSpec) -> Result<Box<dyn ScriptEngine>, EngineError>,
}

impl PartialEq for EngineEntry {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.extensions == other.extensions
    }
}

impl std::fmt::Debug for EngineEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EngineEntry")
            .field("name", &self.name)
            .field("extensions", &self.extensions)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// All engines compiled into this binary.
pub fn engines() -> &'static [EngineEntry] {
    &[
        #[cfg(feature = "engine-luajit")]
        EngineEntry {
            name: "luajit",
            extensions: &["lua"],
            description: "LuaJIT 2.1 (vendored)",
            factory: lua::factory,
        },
        #[cfg(feature = "engine-lua54")]
        EngineEntry {
            name: "lua54",
            extensions: &["lua"],
            description: "Lua 5.4 (vendored)",
            factory: lua::factory,
        },
        #[cfg(feature = "engine-stub")]
        EngineEntry {
            name: "stub",
            extensions: &["stub"],
            description: "Minimal engine used by tests",
            factory: stub::factory,
        },
        #[cfg(feature = "engine-quickjs")]
        EngineEntry {
            name: "quickjs",
            extensions: &["js"],
            description: "QuickJS (rquickjs)",
            factory: quickjs::factory,
        },
    ]
}

/// Finds the compiled engine with the given `-e` name.
pub fn find(name: &str) -> Option<&'static EngineEntry> {
    engines().iter().find(|entry| entry.name == name)
}

/// Finds the compiled engine handling a script file extension.
pub fn find_by_extension(script: &str) -> Option<&'static EngineEntry> {
    let extension = Path::new(script).extension()?.to_str()?;
    engines()
        .iter()
        .find(|entry| entry.extensions.contains(&extension))
}

/// Chooses the engine for a run.
///
/// Dispatch follows the wrkrs rules: `-e` overrides, otherwise the
/// script extension decides. Returns `None` when no script was given
/// and no engine was forced, which leaves the host with the default
/// request path.
pub fn select(
    script: Option<&str>,
    engine: Option<&str>,
) -> Result<Option<&'static EngineEntry>, SelectionError> {
    let selected = match engine {
        Some(name) => Some(find(name).ok_or_else(|| unknown_engine(name))?),
        None => None,
    };

    match (script, selected) {
        (None, Some(_)) => Err(SelectionError::EngineWithoutScript),
        (None, None) => Ok(None),
        (Some(_), Some(entry)) => Ok(Some(entry)),
        (Some(script), None) => find_by_extension(script)
            .map(Some)
            .ok_or_else(|| unknown_extension(script)),
    }
}

/// Builds the unknown engine error, with a rebuild hint when the name
/// belongs to a project engine.
fn unknown_engine(name: &str) -> SelectionError {
    if PROJECT_ENGINES.contains(&name) {
        SelectionError::KnownEngineNotCompiled(name.to_owned())
    } else {
        SelectionError::UnknownEngine(name.to_owned(), engine_list())
    }
}

/// Builds the dispatch error for a script no engine can take.
fn unknown_extension(script: &str) -> SelectionError {
    match Path::new(script)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension) => SelectionError::UnknownExtension(extension.to_owned(), engine_list()),
        None => SelectionError::NoExtension(script.to_owned(), engine_list()),
    }
}

/// Renders the compiled engine list used in error messages.
fn engine_list() -> String {
    let entries = engines();
    if entries.is_empty() {
        return "(none)".to_owned();
    }
    entries
        .iter()
        .map(|entry| {
            let extensions: Vec<String> = entry
                .extensions
                .iter()
                .map(|extension| format!(".{extension}"))
                .collect();
            format!("{} ({})", entry.name, extensions.join(" "))
        })
        .collect::<Vec<String>>()
        .join(", ")
}

#[cfg(all(test, any(feature = "engine-luajit", feature = "engine-lua54")))]
mod lua_registry_tests {
    use super::{engines, find, find_by_extension};

    #[test]
    fn dispatches_lua_scripts_to_the_compiled_lua_engine() {
        let entry = find_by_extension("bench.lua").unwrap();
        #[cfg(feature = "engine-luajit")]
        assert_eq!(entry.name, "luajit");
        #[cfg(feature = "engine-lua54")]
        assert_eq!(entry.name, "lua54");
    }

    #[test]
    fn finds_the_lua_engine_by_flag() {
        #[cfg(feature = "engine-luajit")]
        assert_eq!(find("luajit").unwrap().extensions, &["lua"]);
        #[cfg(feature = "engine-lua54")]
        assert_eq!(find("lua54").unwrap().extensions, &["lua"]);
    }

    #[test]
    fn describes_the_lua_engine() {
        let lua = engines()
            .iter()
            .find(|entry| entry.extensions.contains(&"lua"))
            .unwrap();
        assert!(!lua.description.is_empty());
    }
}

#[cfg(all(test, feature = "engine-quickjs"))]
mod quickjs_registry_tests {
    use super::find_by_extension;

    #[test]
    fn dispatches_js_scripts_to_the_quickjs_engine() {
        assert_eq!(find_by_extension("bench.js").unwrap().name, "quickjs");
    }
}

#[cfg(all(test, feature = "engine-stub"))]
mod stub_tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Mutex};

    use super::stub::factory;
    use super::{SelectionError, engines, find, find_by_extension, select};
    use wrkrs_engine::{ResolveApi, ScriptSpec, UrlRef, Value};

    use std::io;

    fn spec() -> ScriptSpec {
        ScriptSpec {
            url: "http://example.test:8080/".to_owned(),
            parts: UrlRef {
                scheme: Some("http".to_owned()),
                host: Some("example.test".to_owned()),
                port: Some("8080".to_owned()),
                path: "/".to_owned(),
            },
            script: None,
            headers: vec![],
        }
    }

    struct FakeResolver {
        reachable: Vec<SocketAddr>,
        all: Vec<SocketAddr>,
    }

    impl ResolveApi for FakeResolver {
        fn lookup(&self, _host: &str, _service: &str) -> io::Result<Vec<SocketAddr>> {
            Ok(self.all.clone())
        }

        fn connect(&self, addr: &SocketAddr) -> bool {
            self.reachable.contains(addr)
        }
    }

    fn address(last: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, last)), 8080)
    }

    #[test]
    fn registers_the_stub_engine() {
        assert!(engines().iter().any(|entry| entry.name == "stub"));
    }

    #[test]
    fn finds_stub_by_name() {
        assert_eq!(find("stub").unwrap().name, "stub");
        assert!(find("lua").is_none());
    }

    #[test]
    fn finds_stub_by_extension() {
        assert_eq!(find_by_extension("bench.stub").unwrap().name, "stub");
        #[cfg(not(any(feature = "engine-luajit", feature = "engine-lua54")))]
        assert!(find_by_extension("bench.lua").is_none());
        assert!(find_by_extension("bench").is_none());
    }

    #[test]
    fn factory_builds_the_default_request() {
        let mut engine = factory(&spec()).unwrap();
        assert_eq!(
            engine.request().unwrap(),
            b"GET / HTTP/1.1\r\nHost: example.test:8080\r\n\r\n"
        );
    }

    #[test]
    fn resolve_drops_unreachable_addresses() {
        let reachable = address(1);
        let resolver = FakeResolver {
            all: vec![address(9), reachable, address(5)],
            reachable: vec![reachable],
        };
        let mut engine = factory(&spec()).unwrap();
        let resolved = engine
            .resolve("example.test", "8080", Arc::new(resolver))
            .unwrap();
        assert_eq!(resolved, vec![reachable]);
    }

    #[test]
    fn globals_round_trip_and_default_to_null() {
        let mut engine = factory(&spec()).unwrap();
        assert_eq!(engine.get_global("id").unwrap(), Value::Null);
        engine.set_global("id", &Value::Int(7)).unwrap();
        assert_eq!(engine.get_global("id").unwrap(), Value::Int(7));
    }

    #[test]
    fn reports_static_capabilities() {
        let engine = factory(&spec()).unwrap();
        let capabilities = engine.capabilities();
        assert!(capabilities.is_static);
        assert!(!capabilities.wants_response);
        assert!(!capabilities.has_delay);
        assert!(!capabilities.has_done);
    }

    #[test]
    fn init_accepts_a_shared_thread_handle() {
        struct FakeThread {
            stopped: Mutex<bool>,
        }

        impl wrkrs_engine::ThreadApi for FakeThread {
            fn addr(&self) -> Option<SocketAddr> {
                None
            }

            fn set_addr(&self, _addr: SocketAddr) {}

            fn stop(&self) {
                *self.stopped.lock().unwrap_or_else(|p| p.into_inner()) = true;
            }

            fn get_global(&self, _name: &str) -> Result<Value, wrkrs_engine::EngineError> {
                Ok(Value::Null)
            }

            fn set_global(
                &self,
                _name: &str,
                _value: &Value,
            ) -> Result<(), wrkrs_engine::EngineError> {
                Ok(())
            }
        }

        let handle = Arc::new(FakeThread {
            stopped: Mutex::new(false),
        });
        let mut engine = factory(&spec()).unwrap();
        engine.init(handle, &[]).unwrap();
        assert_eq!(engine.delay(), 0);
    }

    #[test]
    fn select_dispatches_by_extension() {
        let entry = select(Some("bench.stub"), None).unwrap().unwrap();
        assert_eq!(entry.name, "stub");
    }

    #[test]
    fn select_returns_none_without_a_script() {
        assert!(select(None, None).unwrap().is_none());
    }

    #[test]
    fn select_lets_the_engine_flag_override_the_extension() {
        let entry = select(Some("anything.txt"), Some("stub")).unwrap().unwrap();
        assert_eq!(entry.name, "stub");
    }

    #[test]
    fn select_rejects_the_engine_flag_without_a_script() {
        assert_eq!(
            select(None, Some("stub")),
            Err(SelectionError::EngineWithoutScript)
        );
        assert_eq!(
            select(None, Some("stub")).unwrap_err().to_string(),
            "option -e requires a script (-s)"
        );
    }

    #[test]
    fn select_reports_project_engines_with_a_rebuild_hint() {
        // Whichever Lua runtime is absent stands in for the not compiled
        // project engine.
        #[cfg(feature = "engine-luajit")]
        let missing = "lua54";
        #[cfg(feature = "engine-lua54")]
        let missing = "luajit";
        let error = select(Some("bench.stub"), Some(missing)).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("engine '{missing}' not compiled in, rebuild with --features engine-{missing}")
        );
    }

    #[test]
    fn select_reports_unknown_engines_with_the_table() {
        let error = select(Some("bench.stub"), Some("lua55")).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "engine 'lua55' not compiled in, compiled engines: {}",
                super::engine_list()
            )
        );
    }

    #[test]
    fn select_reports_unhandled_extensions_with_the_table() {
        let error = select(Some("bench.txt"), None).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "no engine handles the 'txt' extension, compiled engines: {}",
                super::engine_list()
            )
        );
    }

    #[test]
    fn select_reports_scripts_without_an_extension() {
        let error = select(Some("bench"), None).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "'bench' has no extension, compiled engines: {}",
                super::engine_list()
            )
        );
    }
}
