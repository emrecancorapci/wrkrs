use std::io;
use std::net::SocketAddr;

use crate::error::EngineError;
use crate::value::Value;

/// Host side name resolution and connectivity probing.
///
/// Backs the `wrk.lookup()` and `wrk.connect()` script functions with the
/// same semantics as wrk's C implementation.
pub trait ResolveApi: Send + Sync {
    /// Resolves a host and service pair to addresses.
    ///
    /// Matches POSIX `getaddrinfo` with `AF_UNSPEC` and `SOCK_STREAM`.
    /// The result keeps resolver order because scripts index into it.
    fn lookup(&self, host: &str, service: &str) -> io::Result<Vec<SocketAddr>>;

    /// Probes whether a TCP connection to the address succeeds.
    fn connect(&self, addr: &SocketAddr) -> bool;
}

/// Host handle to one benchmark thread, shared with script environments.
///
/// The main environment holds it during setup and done, and a thread
/// environment keeps a reference for the whole run. This matches the
/// lifetime of the `thread` userdata and `wrk.thread` script objects.
///
/// All methods take `&self`. The host provides the synchronization
/// because the phases that touch a thread overlap.
pub trait ThreadApi: Send + Sync {
    /// The address new connections of this thread use.
    fn addr(&self) -> Option<SocketAddr>;
    /// Replaces the address new connections of this thread use.
    fn set_addr(&self, addr: SocketAddr);
    /// Stops the thread event loop.
    fn stop(&self);
    /// Reads a global from the thread environment.
    fn get_global(&self, name: &str) -> Result<Value, EngineError>;
    /// Writes a global in the thread environment.
    fn set_global(&self, name: &str, value: &Value) -> Result<(), EngineError>;
}

/// Read only view of one recorded statistic.
///
/// The host records latency and request rate statistics and scripts read
/// them through the stats objects passed to `done()`.
pub trait StatsView: Send + Sync {
    /// The smallest recorded value.
    fn min(&self) -> u64;
    /// The largest recorded value.
    fn max(&self) -> u64;
    /// The arithmetic mean of the recorded values.
    fn mean(&self) -> f64;
    /// The standard deviation of the recorded values.
    fn stdev(&self) -> f64;
    /// The value at the given percentile between 0 and 100.
    fn percentile(&self, percentile: f64) -> u64;
    /// The number of histogram slots that hold recorded values.
    fn popcount(&self) -> u64;
    /// The raw value and its count at a zero based histogram slot.
    fn value_at(&self, slot: u64) -> (u64, u64);
}

/// Socket error counters reported to the done callback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ErrorCounts {
    /// Failed connection attempts.
    pub connect: u64,
    /// Socket read errors.
    pub read: u64,
    /// Socket write errors.
    pub write: u64,
    /// HTTP status codes above 399.
    pub status: u64,
    /// Request timeouts.
    pub timeout: u64,
}

/// Run totals reported to the done callback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    /// Run duration in microseconds.
    pub duration: u64,
    /// Completed requests.
    pub requests: u64,
    /// Bytes received.
    pub bytes: u64,
    /// Socket error counters.
    pub errors: ErrorCounts,
}
