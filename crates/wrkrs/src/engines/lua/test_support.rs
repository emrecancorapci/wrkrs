//! Shared fakes for the Lua engine tests.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;

use wrkrs_engine::{EngineError, ResolveApi, StatsView, ThreadApi, Value};

/// Records thread state the way the host would.
#[derive(Default)]
pub struct FakeThread {
    addr: Mutex<Option<SocketAddr>>,
    stopped: Mutex<bool>,
    globals: Mutex<HashMap<String, Value>>,
}

impl FakeThread {
    pub fn stopped(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl ThreadApi for FakeThread {
    fn addr(&self) -> Option<SocketAddr> {
        *self.addr.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_addr(&self, addr: SocketAddr) {
        *self.addr.lock().unwrap_or_else(|p| p.into_inner()) = Some(addr);
    }

    fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|p| p.into_inner()) = true;
    }

    fn get_global(&self, name: &str) -> Result<Value, EngineError> {
        Ok(self
            .globals
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn set_global(&self, name: &str, value: &Value) -> Result<(), EngineError> {
        self.globals
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name.to_owned(), value.clone());
        Ok(())
    }
}

/// Resolves a fixed address list with a configurable reachable set.
pub struct FakeResolver {
    pub reachable: Vec<SocketAddr>,
}

impl Default for FakeResolver {
    fn default() -> Self {
        FakeResolver {
            reachable: vec![address(1)],
        }
    }
}

impl ResolveApi for FakeResolver {
    fn lookup(&self, _host: &str, _service: &str) -> io::Result<Vec<SocketAddr>> {
        Ok(vec![address(9), address(1), address(5)])
    }

    fn connect(&self, addr: &SocketAddr) -> bool {
        self.reachable.contains(addr)
    }
}

/// Failing resolver for the error path.
pub struct FailingResolver;

impl ResolveApi for FailingResolver {
    fn lookup(&self, _host: &str, _service: &str) -> io::Result<Vec<SocketAddr>> {
        Err(io::Error::other("name or service not known"))
    }

    fn connect(&self, _addr: &SocketAddr) -> bool {
        true
    }
}

/// Fixed statistics for the done callback tests.
pub struct FakeStats;

impl StatsView for FakeStats {
    fn min(&self) -> u64 {
        100
    }

    fn max(&self) -> u64 {
        900
    }

    fn mean(&self) -> f64 {
        250.5
    }

    fn stdev(&self) -> f64 {
        12.25
    }

    fn percentile(&self, percentile: f64) -> u64 {
        (percentile as u64) + 1
    }

    fn popcount(&self) -> u64 {
        7
    }

    fn value_at(&self, slot: u64) -> (u64, u64) {
        (100 + slot, 10 * slot)
    }
}

/// A loopback address with the given last octet on port 8080.
pub fn address(last: u8) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, last)), 8080)
}
