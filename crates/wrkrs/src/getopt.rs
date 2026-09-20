//! A getopt style parser for the wrk command line.
//!
//! Reproduces the glibc getopt_long behaviors wrk depends on: short
//! option clustering, attached values, long options with unambiguous
//! prefix matching, and the glibc error message shapes with the
//! program name as prefix.

/// Definition of one command line option.
#[derive(Debug)]
pub struct OptionDef {
    /// The short flag character.
    pub short: char,
    /// The long option name without the leading dashes.
    pub long: &'static str,
    /// Whether the option consumes a value.
    pub value: bool,
}

/// One parsed option occurrence.
#[derive(Debug, PartialEq)]
pub struct ParsedOption {
    /// The short character identifying the option.
    pub short: char,
    /// The value when the option takes one.
    pub value: Option<String>,
}

/// Parses arguments with getopt semantics.
///
/// Returns the options in order together with the positional
/// arguments. The error string is the glibc shaped message for the
/// first failure, wrk stops at the first one. Everything after `--`
/// becomes positional without further parsing and a bare dash counts
/// as positional.
pub fn parse(
    program: &str,
    args: &[String],
    options: &[OptionDef],
) -> Result<(Vec<ParsedOption>, Vec<String>), String> {
    let mut parsed = Vec::new();
    let mut positional = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;

        if arg == "--" {
            positional.extend_from_slice(&args[index..]);
            break;
        }

        if let Some(body) = arg.strip_prefix("--") {
            let (name, attached) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (body, None),
            };
            let definition = match_long(program, name, options)?;
            match (definition.value, attached) {
                (false, Some(_)) => {
                    return Err(format!(
                        "{program}: option '--{name}' doesn't allow an argument"
                    ));
                }
                (false, None) => parsed.push(valueless(definition.short)),
                (true, Some(value)) => parsed.push(ParsedOption {
                    short: definition.short,
                    value: Some(value.to_owned()),
                }),
                (true, None) => {
                    let value = args.get(index).ok_or_else(|| {
                        format!("{program}: option '--{name}' requires an argument")
                    })?;
                    index += 1;
                    parsed.push(ParsedOption {
                        short: definition.short,
                        value: Some(value.clone()),
                    });
                }
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            // A short cluster, the value may ride attached after its
            // option character.
            let cluster: Vec<char> = arg.chars().skip(1).collect();
            let mut position = 0;
            while position < cluster.len() {
                let flag = cluster[position];
                position += 1;
                let definition = options
                    .iter()
                    .find(|definition| definition.short == flag)
                    .ok_or_else(|| format!("{program}: invalid option -- '{flag}'"))?;
                if definition.value {
                    let rest: String = cluster[position..].iter().collect();
                    let value = if !rest.is_empty() {
                        rest
                    } else {
                        let value = args.get(index).ok_or_else(|| {
                            format!("{program}: option requires an argument -- '{flag}'")
                        })?;
                        index += 1;
                        value.clone()
                    };
                    parsed.push(ParsedOption {
                        short: flag,
                        value: Some(value),
                    });
                    break;
                }
                parsed.push(valueless(flag));
            }
        } else {
            positional.push(arg.to_owned());
        }
    }

    Ok((parsed, positional))
}

/// Builds a valueless option.
fn valueless(short: char) -> ParsedOption {
    ParsedOption { short, value: None }
}

/// Matches a long option name exactly or as an unambiguous prefix.
fn match_long<'a>(
    program: &str,
    name: &str,
    options: &'a [OptionDef],
) -> Result<&'a OptionDef, String> {
    if let Some(exact) = options.iter().find(|definition| definition.long == name) {
        return Ok(exact);
    }
    let candidates: Vec<&OptionDef> = options
        .iter()
        .filter(|definition| definition.long.starts_with(name))
        .collect();
    match candidates.as_slice() {
        [] => Err(format!("{program}: unrecognized option '--{name}'")),
        [only] => Ok(only),
        many => {
            let possibilities = many
                .iter()
                .map(|definition| format!("'--{}'", definition.long))
                .collect::<Vec<String>>()
                .join(" ");
            Err(format!(
                "{program}: option '--{name}' is ambiguous; possibilities: {possibilities}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OptionDef, parse};

    fn options() -> Vec<OptionDef> {
        vec![
            OptionDef {
                short: 't',
                long: "threads",
                value: true,
            },
            OptionDef {
                short: 'c',
                long: "connections",
                value: true,
            },
            OptionDef {
                short: 'L',
                long: "latency",
                value: false,
            },
            OptionDef {
                short: 'T',
                long: "timeout",
                value: true,
            },
        ]
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn shorts(parsed: &[super::ParsedOption]) -> Vec<char> {
        parsed.iter().map(|option| option.short).collect()
    }

    #[test]
    fn reads_separated_and_attached_short_values() {
        let (parsed, positional) = parse("wrk", &args(&["-t", "2", "-c10"]), &options()).unwrap();
        assert_eq!(shorts(&parsed), ['t', 'c']);
        assert_eq!(parsed[0].value.as_deref(), Some("2"));
        assert_eq!(parsed[1].value.as_deref(), Some("10"));
        assert!(positional.is_empty());
    }

    #[test]
    fn walks_short_clusters() {
        let (parsed, _) = parse("wrk", &args(&["-Lt2"]), &options()).unwrap();
        assert_eq!(shorts(&parsed), ['L', 't']);
        assert_eq!(parsed[1].value.as_deref(), Some("2"));
    }

    #[test]
    fn reads_long_options_in_both_forms() {
        let (parsed, _) =
            parse("wrk", &args(&["--threads=2", "--timeout", "3"]), &options()).unwrap();
        assert_eq!(parsed[0].value.as_deref(), Some("2"));
        assert_eq!(parsed[1].value.as_deref(), Some("3"));
    }

    #[test]
    fn matches_unique_long_prefixes() {
        let (parsed, _) = parse("wrk", &args(&["--thr", "4"]), &options()).unwrap();
        assert_eq!(parsed[0].short, 't');
        assert_eq!(parsed[0].value.as_deref(), Some("4"));
    }

    #[test]
    fn exact_match_beats_prefix_overlap() {
        let (parsed, _) = parse("wrk", &args(&["--timeout", "2"]), &options()).unwrap();
        assert_eq!(parsed[0].short, 'T');
    }

    #[test]
    fn keeps_positional_order() {
        let (_, positional) = parse(
            "wrk",
            &args(&["url", "-L", "-", "--", "-x", "tail"]),
            &options(),
        )
        .unwrap();
        assert_eq!(positional, args(&["url", "-", "-x", "tail"]));
    }

    #[test]
    fn rejects_unknown_short_options() {
        let error = parse("wrkrs", &args(&["-z"]), &options()).unwrap_err();
        assert_eq!(error, "wrkrs: invalid option -- 'z'");
    }

    #[test]
    fn rejects_missing_short_values() {
        let error = parse("wrkrs", &args(&["-t"]), &options()).unwrap_err();
        assert_eq!(error, "wrkrs: option requires an argument -- 't'");
    }

    #[test]
    fn rejects_unknown_long_options() {
        let error = parse("wrkrs", &args(&["--wat"]), &options()).unwrap_err();
        assert_eq!(error, "wrkrs: unrecognized option '--wat'");
    }

    #[test]
    fn rejects_ambiguous_long_prefixes() {
        let error = parse("wrkrs", &args(&["--t", "2"]), &options()).unwrap_err();
        assert_eq!(
            error,
            "wrkrs: option '--t' is ambiguous; possibilities: '--threads' '--timeout'"
        );
    }

    #[test]
    fn rejects_missing_long_values() {
        let error = parse("wrkrs", &args(&["--threads"]), &options()).unwrap_err();
        assert_eq!(error, "wrkrs: option '--threads' requires an argument");
    }

    #[test]
    fn rejects_values_on_flag_options() {
        let error = parse("wrkrs", &args(&["--latency=yes"]), &options()).unwrap_err();
        assert_eq!(error, "wrkrs: option '--latency' doesn't allow an argument");
    }
}
