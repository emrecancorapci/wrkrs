/// An error produced while creating or driving a scripting engine.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EngineError {
    /// The engine failed to load the script file.
    ///
    /// The message is the raw error reported by the scripting VM, for
    /// example `bench.lua:3: '=' expected near '<eof>'`.
    #[error("script load failed: {0}")]
    Load(String),
    /// The script raised an error inside a callback.
    ///
    /// The message is the raw error reported by the scripting VM.
    #[error("script error: {0}")]
    Runtime(String),
    /// A value cannot be transferred into or out of a thread environment.
    ///
    /// Only nil, boolean, number, string, and tables of the same are
    /// transferable. This mirrors the wrk `thread:get()` and
    /// `thread:set()` restriction.
    #[error("cannot transfer {0} to thread")]
    UnsupportedValue(String),
    /// Host name resolution failed during a script lookup call.
    #[error("name resolution failed: {0}")]
    Resolve(String),
}

impl EngineError {
    /// The raw scripting VM message carried by this error.
    ///
    /// Hosts compose wrk compatible diagnostics from this message. The
    /// [`Display`](std::fmt::Display) form adds a prefix and is meant for
    /// logs that are not parity constrained.
    pub fn raw_message(&self) -> &str {
        match self {
            EngineError::Load(message)
            | EngineError::Runtime(message)
            | EngineError::UnsupportedValue(message)
            | EngineError::Resolve(message) => message,
        }
    }
}
