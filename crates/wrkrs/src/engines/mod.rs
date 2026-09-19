use std::path::Path;

use wrkrs_engine::{EngineError, ScriptEngine, ScriptSpec};

#[cfg(feature = "engine-stub")]
pub mod stub;

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

/// All engines compiled into this binary.
pub fn engines() -> &'static [EngineEntry] {
    &[
        #[cfg(feature = "engine-stub")]
        EngineEntry {
            name: "stub",
            extensions: &["stub"],
            description: "Minimal engine used by tests",
            factory: stub::factory,
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

#[cfg(all(test, not(feature = "engine-stub")))]
mod empty_registry_tests {
    use super::{find, find_by_extension};

    #[test]
    fn find_returns_none_for_any_name() {
        assert!(find("luajit").is_none());
        assert!(find("stub").is_none());
    }

    #[test]
    fn find_by_extension_returns_none_for_any_file() {
        assert!(find_by_extension("bench.lua").is_none());
        assert!(find_by_extension("bench").is_none());
    }
}

#[cfg(all(test, feature = "engine-stub"))]
mod stub_tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Mutex};

    use super::stub::factory;
    use super::{engines, find, find_by_extension};
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
        let table = engines();
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].name, "stub");
        assert_eq!(table[0].extensions, &["stub"]);
    }

    #[test]
    fn finds_stub_by_name() {
        assert_eq!(find("stub").unwrap().name, "stub");
        assert!(find("lua").is_none());
    }

    #[test]
    fn finds_stub_by_extension() {
        assert_eq!(find_by_extension("bench.stub").unwrap().name, "stub");
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
        let resolved = engine.resolve("example.test", "8080", &resolver).unwrap();
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
}
