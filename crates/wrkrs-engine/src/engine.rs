use std::net::SocketAddr;
use std::sync::Arc;

use crate::api::{ResolveApi, StatsView, Summary, ThreadApi};
use crate::capabilities::Capabilities;
use crate::error::EngineError;
use crate::spec::ScriptSpec;
use crate::value::Value;
/// One scripting environment the host drives through the wrk phases.
///
/// The host builds a main environment for resolve, setup, and done plus
/// one environment per benchmark thread for the running phase.
///
/// Implementations must be `Send`. The host moves thread environments
/// into their worker threads.
pub trait ScriptEngine: Send {
    /// Builds one environment from a spec.
    ///
    /// Loads the script when the spec carries one and installs the wrk
    /// table with URL parts and headers. Script load failures follow wrk
    /// behavior: the engine reports the error on stderr and keeps the
    /// environment usable with defaults.
    ///
    /// This method is not object safe. Registries wrap it in a factory
    /// function pointer.
    fn create(spec: &ScriptSpec) -> Result<Self, EngineError>
    where
        Self: Sized;

    /// Resolves the target and returns the reachable addresses.
    ///
    /// Runs the script visible resolve step: look up every address, drop
    /// the ones that refuse a connection probe, and return the rest in
    /// resolver order. An empty result means the target is unreachable.
    ///
    /// The resolver is shared because scripts can call the lookup
    /// functions at any time, matching wrk where the C functions stay
    /// installed for the whole run.
    fn resolve(
        &mut self,
        host: &str,
        service: &str,
        resolver: Arc<dyn ResolveApi>,
    ) -> Result<Vec<SocketAddr>, EngineError>;

    /// Runs the setup phase for one thread.
    ///
    /// Called on the main environment once per thread, before that thread
    /// starts. Scripts receive the thread object and may set its address,
    /// transfer values into the thread environment, or stop the thread.
    ///
    /// The handle is shared because scripts keep the thread object past
    /// the callback, wrk's setup.lua stores it and reads it in done.
    fn setup(&mut self, thread: Arc<dyn ThreadApi>) -> Result<(), EngineError>;

    /// Runs the running phase entry on a thread environment.
    ///
    /// Stores the shared thread handle as the script visible thread
    /// object, then calls the script init callback with the extra command
    /// line arguments that followed `--`.
    fn init(&mut self, thread: Arc<dyn ThreadApi>, args: &[String]) -> Result<(), EngineError>;

    /// Returns the milliseconds to wait before sending the next request.
    ///
    /// Matches wrk: only called while the script supplies a delay
    /// callback, and a callback error is fatal the way wrk's unprotected
    /// call is.
    fn delay(&mut self) -> u64;

    /// Builds the next HTTP request bytes.
    fn request(&mut self) -> Result<Vec<u8>, EngineError>;

    /// Delivers one parsed HTTP response.
    ///
    /// Called only while the script wants responses. Header order is
    /// preserved. Duplicate header names appear once and the last value
    /// wins, matching the table semantics scripts observe.
    fn response(
        &mut self,
        status: u16,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(), EngineError>;

    /// Delivers the run results to the done callback.
    ///
    /// Called on the main environment after every thread finished. The
    /// stats views are shared because scripts can keep the stats objects,
    /// matching wrk where the C structs outlive the call.
    fn done(
        &mut self,
        summary: &Summary,
        latency: Arc<dyn StatsView>,
        requests: Arc<dyn StatsView>,
    ) -> Result<(), EngineError>;

    /// Reports what the loaded script demands from the run loop.
    fn capabilities(&self) -> Capabilities;

    /// Reads one global from this environment.
    ///
    /// Backs the cross environment value transfer. A value the transfer
    /// rules reject surfaces as
    /// [`EngineError::UnsupportedValue`](crate::EngineError::UnsupportedValue).
    fn get_global(&self, name: &str) -> Result<Value, EngineError>;

    /// Writes one global in this environment.
    fn set_global(&mut self, name: &str, value: &Value) -> Result<(), EngineError>;
}
