/// What the loaded script demands from the run loop.
///
/// Mirrors the probes wrk takes after the first thread initializes.
/// Engines must report these honestly because the host changes
/// connection behavior based on them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// The request never changes, so the host may cache and reuse it.
    pub is_static: bool,
    /// The script wants response status, headers, and body.
    pub wants_response: bool,
    /// The script supplies a per request delay.
    pub has_delay: bool,
    /// The script implements the done callback.
    pub has_done: bool,
}
