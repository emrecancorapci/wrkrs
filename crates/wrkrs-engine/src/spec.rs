use std::path::PathBuf;

/// The URL parts wrk exposes to scripts.
///
/// Field semantics follow wrk's C URL parser:
///
/// - `scheme` and `host` are `None` when the URL carries none
/// - `port` stays the raw digits because wrk installs it as a string,
///   not a number
/// - `path` takes everything from the path offset to the end of the
///   URL, query string and fragment included, because script.c slices
///   the tail instead of the field length. A URL without a path keeps
///   the default `/` and loses its query
/// - an IPv6 host excludes the brackets, wrk.lua adds them back when
///   it builds the Host header
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlRef {
    /// URL scheme, for example `http`.
    pub scheme: Option<String>,
    /// URL host, without brackets and port.
    pub host: Option<String>,
    /// Raw port digits from the URL when present.
    pub port: Option<String>,
    /// URL path, `/` when the URL carries none.
    pub path: String,
}

/// Everything an engine needs to build one scripting environment.
///
/// The host builds a main environment for resolve, setup, and done plus
/// one environment per benchmark thread, all from the same spec.
#[derive(Debug, Clone)]
pub struct ScriptSpec {
    /// The benchmark URL exactly as given on the command line.
    pub url: String,
    /// Parsed URL parts the engine installs into the wrk table.
    pub parts: UrlRef,
    /// Script file to run, `None` when no script was given.
    pub script: Option<PathBuf>,
    /// Request headers from `-H`, split into name and value at the first
    /// `": "` separator. Headers without that separator are dropped.
    pub headers: Vec<(String, String)>,
}
