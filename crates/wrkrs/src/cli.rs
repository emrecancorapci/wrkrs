use crate::engines::EngineEntry;

/// Engine related flags extracted from the command line.
///
/// The scanner recognizes the engine flags anywhere before `--` and
/// leaves every other argument alone. The full getopt compatible
/// argument parser lands with the core port and replaces this scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineFlags {
    /// Value of `-e` or `--engine`.
    pub engine: Option<String>,
    /// Value of `-s` or `--script`.
    pub script: Option<String>,
    /// `-E` or `--engines` was given.
    pub list_engines: bool,
}

/// Error scanning the engine flags.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlagError {
    /// A flag that takes a value sits at the end of the arguments.
    #[error("option requires an argument -- '{0}'")]
    MissingArgument(&'static str),
}

/// Scans the engine flags from command line arguments.
///
/// Accepted forms: `-e name`, `-ename`, `--engine name`, `--engine=name`
/// and the `-s` equivalents, plus `-E` and `--engines`. Scanning stops
/// at `--` because everything after it belongs to the script.
pub fn scan(args: &[String]) -> Result<EngineFlags, FlagError> {
    let mut flags = EngineFlags::default();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        index += 1;

        if arg == "--" {
            break;
        }

        if let Some(value) = arg.strip_prefix("--engine=") {
            flags.engine = Some(value.to_owned());
        } else if arg == "--engine" {
            flags.engine = Some(take_value(args, &mut index, "engine")?);
        } else if let Some(value) = arg.strip_prefix("--script=") {
            flags.script = Some(value.to_owned());
        } else if arg == "--script" {
            flags.script = Some(take_value(args, &mut index, "script")?);
        } else if arg == "--engines" || arg == "-E" {
            flags.list_engines = true;
        } else if let Some(value) = arg.strip_prefix("-e") {
            flags.engine = Some(if value.is_empty() {
                take_value(args, &mut index, "e")?
            } else {
                value.to_owned()
            });
        } else if let Some(value) = arg.strip_prefix("-s") {
            flags.script = Some(if value.is_empty() {
                take_value(args, &mut index, "s")?
            } else {
                value.to_owned()
            });
        }
    }

    Ok(flags)
}

/// Renders the engine listing printed by `-E`.
pub fn engine_listing(entries: &[EngineEntry]) -> String {
    let mut listing = String::new();
    for entry in entries {
        let extensions: Vec<String> = entry
            .extensions
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect();
        listing.push_str(&format!(
            "  {:<10} {:<7} {}\n",
            entry.name,
            extensions.join(" "),
            entry.description
        ));
    }
    listing
}

/// Takes the value that follows a flag from the argument list.
fn take_value(
    args: &[String],
    index: &mut usize,
    option: &'static str,
) -> Result<String, FlagError> {
    match args.get(*index) {
        Some(value) => {
            *index += 1;
            Ok(value.clone())
        }
        None => Err(FlagError::MissingArgument(option)),
    }
}

#[cfg(test)]
mod tests {
    use super::{EngineFlags, FlagError, scan};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn flags(engine: Option<&str>, script: Option<&str>, list_engines: bool) -> EngineFlags {
        EngineFlags {
            engine: engine.map(str::to_owned),
            script: script.map(str::to_owned),
            list_engines,
        }
    }

    #[test]
    fn reads_separated_short_flags() {
        assert_eq!(
            scan(&args(&["-e", "stub", "-s", "bench.stub"])).unwrap(),
            flags(Some("stub"), Some("bench.stub"), false)
        );
    }

    #[test]
    fn reads_attached_short_values() {
        assert_eq!(
            scan(&args(&["-estub", "-sbench.stub"])).unwrap(),
            flags(Some("stub"), Some("bench.stub"), false)
        );
    }

    #[test]
    fn reads_long_flags_with_and_without_equals() {
        assert_eq!(
            scan(&args(&["--engine=stub", "--script", "bench.stub"])).unwrap(),
            flags(Some("stub"), Some("bench.stub"), false)
        );
    }

    #[test]
    fn reads_the_list_flag_in_both_forms() {
        assert_eq!(scan(&args(&["-E"])).unwrap(), flags(None, None, true));
        assert_eq!(
            scan(&args(&["--engines"])).unwrap(),
            flags(None, None, true)
        );
    }

    #[test]
    fn stops_at_the_separator() {
        assert_eq!(
            scan(&args(&["-e", "stub", "--", "-E", "-elua54"])).unwrap(),
            flags(Some("stub"), None, false)
        );
    }

    #[test]
    fn ignores_other_arguments() {
        assert_eq!(
            scan(&args(&["-t2", "-c100", "http://example.test/", "-L"])).unwrap(),
            flags(None, None, false)
        );
    }

    #[test]
    fn rejects_a_missing_flag_value() {
        assert_eq!(scan(&args(&["-e"])), Err(FlagError::MissingArgument("e")));
        assert_eq!(
            scan(&args(&["--engine"])),
            Err(FlagError::MissingArgument("engine"))
        );
        assert_eq!(
            scan(&args(&["-e"])).unwrap_err().to_string(),
            "option requires an argument -- 'e'"
        );
    }

    #[test]
    fn last_value_wins() {
        assert_eq!(
            scan(&args(&["-e", "stub", "-e", "quickjs"])).unwrap(),
            flags(Some("quickjs"), None, false)
        );
    }

    #[cfg(feature = "engine-stub")]
    #[test]
    fn renders_the_engine_listing() {
        use super::engine_listing;
        use crate::engines::engines;
        let listing = engine_listing(engines());
        // The stub entry always renders last with its own line.
        assert!(listing.ends_with("  stub       .stub   Minimal engine used by tests\n"));
    }
}
