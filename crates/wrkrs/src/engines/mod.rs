use std::path::Path;

use wrkrs_engine::{EngineError, ScriptEngine, ScriptSpec};

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
    &[]
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

#[cfg(test)]
mod tests {
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
