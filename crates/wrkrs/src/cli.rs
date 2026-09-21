//! The wrkrs command line: options, defaults, and validation.
//!
//! Ports parse_args from wrk.c including its quirks: `-v` prints the
//! version and keeps parsing, `-h` exits through the usage path with
//! status one, scan failures fall back to usage without a message,
//! and the URL is the first entry of the script arguments.

use std::io::Write;
use std::path::PathBuf;

use crate::getopt::{self, OptionDef};
use crate::parser::parse_url;
use crate::units::{scan_metric, scan_time};
use wrkrs_engine::UrlRef;

/// The report format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputMode {
    /// The byte compatible wrk layout, the 1.0 default.
    #[default]
    Legacy,
    /// The clean v1 layout.
    Modern,
    /// The machine readable v1 object.
    Json,
}

/// Parses an output mode name.
pub fn parse_output_mode(name: &str) -> Option<OutputMode> {
    match name {
        "legacy" => Some(OutputMode::Legacy),
        "modern" => Some(OutputMode::Modern),
        "json" => Some(OutputMode::Json),
        _ => None,
    }
}

/// The run configuration after parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Worker threads.
    pub threads: u64,
    /// Total open connections.
    pub connections: u64,
    /// Run duration in seconds.
    pub duration_s: u64,
    /// Socket timeout in milliseconds.
    pub timeout_ms: u64,
    /// Whether latency percentiles print.
    pub latency: bool,
    /// Script file when given.
    pub script: Option<PathBuf>,
    /// Raw `-H` header strings, split happens when the spec is built.
    pub headers: Vec<String>,
    /// Engine name from `-e` when given.
    pub engine: Option<String>,
    /// The report format, legacy by default.
    pub output: OutputMode,
    /// Where the report lands, stdout by default.
    pub output_file: Option<PathBuf>,
    /// The benchmark URL.
    pub url: String,
    /// Parsed URL parts.
    pub parts: UrlRef,
    /// The positional arguments handed to script init, the URL first.
    pub init_args: Vec<String>,
}

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Run a benchmark.
    Run(Box<Config>),
    /// List the compiled engines and exit zero.
    ListEngines,
    /// Print usage and exit one, an optional message goes to stderr
    /// first.
    Usage(Option<String>),
}

/// The option table in wrk order, the engine flags follow timeout.
const OPTIONS: &[OptionDef] = &[
    OptionDef {
        short: 'c',
        long: "connections",
        value: true,
    },
    OptionDef {
        short: 'd',
        long: "duration",
        value: true,
    },
    OptionDef {
        short: 't',
        long: "threads",
        value: true,
    },
    OptionDef {
        short: 's',
        long: "script",
        value: true,
    },
    OptionDef {
        short: 'H',
        long: "header",
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
    OptionDef {
        short: 'o',
        long: "output",
        value: true,
    },
    OptionDef {
        short: 'O',
        long: "output-file",
        value: true,
    },
    OptionDef {
        short: 'e',
        long: "engine",
        value: true,
    },
    OptionDef {
        short: 'E',
        long: "engines",
        value: false,
    },
    OptionDef {
        short: 'h',
        long: "help",
        value: false,
    },
    OptionDef {
        short: 'v',
        long: "version",
        value: false,
    },
];

/// Parses the arguments.
///
/// Version output goes to `out` the moment `-v` appears, matching the
/// print and continue behavior of wrk.
pub fn parse(program: &str, args: &[String], out: &mut dyn Write) -> Outcome {
    match parse_result(program, args, out) {
        Ok(outcome) => outcome,
        Err(outcome) => outcome,
    }
}

/// Parses into an outcome, errors as usage paths.
fn parse_result(program: &str, args: &[String], out: &mut dyn Write) -> Result<Outcome, Outcome> {
    let (options, positional) =
        getopt::parse(program, args, OPTIONS).map_err(|message| Outcome::Usage(Some(message)))?;

    let mut threads: u64 = 2;
    let mut connections: u64 = 10;
    let mut duration_s: u64 = 10;
    let mut timeout_ms: u64 = 2000;
    let mut latency = false;
    let mut script: Option<PathBuf> = None;
    let mut headers: Vec<String> = Vec::new();
    let mut engine: Option<String> = None;
    let mut list_engines = false;
    let mut output = OutputMode::default();
    let mut output_file: Option<PathBuf> = None;

    for option in &options {
        let value = option.value.as_deref();
        match option.short {
            't' => threads = scan_metric(value.unwrap_or_default()).ok_or(usage())?,
            'c' => connections = scan_metric(value.unwrap_or_default()).ok_or(usage())?,
            'd' => duration_s = scan_time(value.unwrap_or_default()).ok_or(usage())?,
            'T' => {
                let seconds = scan_time(value.unwrap_or_default()).ok_or(usage())?;
                timeout_ms = seconds.saturating_mul(1000);
            }
            's' => script = Some(PathBuf::from(value.unwrap_or_default())),
            'H' => headers.push(value.unwrap_or_default().to_owned()),
            'L' => latency = true,
            'e' => engine = Some(value.unwrap_or_default().to_owned()),
            'E' => list_engines = true,
            'o' => {
                output = parse_output_mode(value.unwrap_or_default()).ok_or_else(|| {
                    Outcome::Usage(Some(format!(
                        "invalid output mode: {} (expected legacy, modern, or json)",
                        value.unwrap_or_default()
                    )))
                })?;
            }
            'O' => output_file = Some(PathBuf::from(value.unwrap_or_default())),
            'v' => {
                // wrk prints the version and keeps parsing.
                let _ = writeln!(out, "{}", version_line());
            }
            _ => return Err(usage()),
        }
    }

    if list_engines {
        return Ok(Outcome::ListEngines);
    }

    let Some(url) = positional.first() else {
        return Err(usage());
    };
    if threads == 0 || duration_s == 0 {
        return Err(usage());
    }
    let Some(parts) = parse_url(url) else {
        return Err(Outcome::Usage(Some(format!("invalid URL: {url}"))));
    };
    if connections == 0 || connections < threads {
        return Err(Outcome::Usage(Some(
            "number of connections must be >= threads".to_owned(),
        )));
    }

    Ok(Outcome::Run(Box::new(Config {
        threads,
        connections,
        duration_s,
        timeout_ms,
        latency,
        script,
        headers,
        engine,
        output,
        output_file,
        url: url.clone(),
        parts,
        // wrk hands the positionals to script init starting at the
        // URL, so scripts see their own arguments from index one.
        init_args: positional,
    })))
}

/// The silent usage failure, matching the scan error path of wrk.
fn usage() -> Outcome {
    Outcome::Usage(None)
}

/// The version line, printed before the copyright notice.
pub fn version_line() -> String {
    // wrk prints the backend and the copyright on one line, the
    // trailing space of its first printf is the separator.
    format!(
        "wrkrs {} [{}] Copyright (C) 2012 Will Glozer",
        version(),
        crate::backend::NAME
    )
}

/// The build version from git describe with a crate fallback.
fn version() -> &'static str {
    option_env!("WRKRS_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// The usage line width of the C usage box: every line pads to it
/// except the last, which the C literal leaves one column short.
const USAGE_WIDTH: usize = 54;

/// Pads one usage line to the box width.
fn pad(line: &str) -> String {
    pad_to(line, USAGE_WIDTH)
}

/// Pads one usage line to a width.
fn pad_to(line: &str, width: usize) -> String {
    let mut line = line.to_owned();
    while line.chars().count() < width {
        line.push(' ');
    }
    line
}

/// Prints the usage text.
pub fn print_usage(out: &mut dyn Write) {
    let lines = [
        "Usage: wrkrs <options> <url>",
        "  Options:",
        "    -c, --connections <N>  Connections to keep open",
        "    -d, --duration    <T>  Duration of test",
        "    -t, --threads     <N>  Number of threads to use",
        "",
        "    -s, --script      <S>  Load script file",
        "    -e, --engine      <E>  Select the script engine",
        "    -E, --engines          List compiled-in engines",
        "    -H, --header      <H>  Add header to request",
        "        --latency          Print latency statistics",
        "        --timeout     <T>  Socket/request timeout",
        "    -o, --output      <M>  legacy, modern, or json",
        "    -O, --output-file <F>  Write report to a file",
        "    -v, --version          Print version details",
        "",
        "  Numeric arguments may include a SI unit (1k, 1M, 1G)",
        "  Time arguments may include a time unit (2s, 2m, 2h)",
    ];
    for (index, line) in lines.iter().enumerate() {
        if index + 1 == lines.len() {
            // The final C literal pads one column short.
            let _ = writeln!(out, "{}", pad_to(line, USAGE_WIDTH - 1));
        } else {
            let _ = writeln!(out, "{}", pad(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{Outcome, OutputMode, parse};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn run(list: &[&str]) -> (Outcome, String) {
        let mut out = Cursor::new(Vec::new());
        let outcome = parse("wrkrs", &args(list), &mut out);
        let printed = String::from_utf8(out.into_inner()).unwrap_or_default();
        (outcome, printed)
    }

    #[test]
    fn applies_the_defaults() {
        let (outcome, _) = run(&["http://host/"]);
        let Outcome::Run(config) = outcome else {
            panic!("expected a run");
        };
        assert_eq!(config.threads, 2);
        assert_eq!(config.connections, 10);
        assert_eq!(config.duration_s, 10);
        assert_eq!(config.timeout_ms, 2000);
        assert!(!config.latency);
        assert!(config.script.is_none());
        assert!(config.engine.is_none());
    }

    #[test]
    fn reads_every_option() {
        let (outcome, _) = run(&[
            "-t",
            "4",
            "-c100",
            "-d",
            "2m",
            "-T",
            "3s",
            "-L",
            "-s",
            "b.lua",
            "-e",
            "quickjs",
            "-H",
            "Accept: text/plain",
            "http://host:8080/x",
        ]);
        let Outcome::Run(config) = outcome else {
            panic!("expected a run");
        };
        assert_eq!(config.threads, 4);
        assert_eq!(config.connections, 100);
        assert_eq!(config.duration_s, 120);
        assert_eq!(config.timeout_ms, 3000);
        assert!(config.latency);
        assert_eq!(config.script, Some("b.lua".into()));
        assert_eq!(config.engine.as_deref(), Some("quickjs"));
        assert_eq!(config.headers, ["Accept: text/plain"]);
        assert_eq!(config.parts.port.as_deref(), Some("8080"));
    }

    #[test]
    fn the_url_leads_the_script_arguments() {
        let (outcome, _) = run(&["http://host/", "--", "one", "two"]);
        let Outcome::Run(config) = outcome else {
            panic!("expected a run");
        };
        assert_eq!(
            config.init_args,
            ["http://host/", "one", "two"].map(str::to_owned)
        );
    }

    #[test]
    fn version_prints_and_keeps_parsing() {
        let (outcome, printed) = run(&["-v", "http://host/"]);
        assert!(matches!(outcome, Outcome::Run(_)));
        assert!(printed.starts_with("wrkrs "), "printed: {printed}");

        // wrk alone with -v falls through to usage because the URL is
        // missing.
        let (outcome, _) = run(&["-v"]);
        assert_eq!(outcome, Outcome::Usage(None));
    }

    #[test]
    fn scan_failures_fall_back_to_usage_silently() {
        let (outcome, _) = run(&["-t", "wat", "http://host/"]);
        assert_eq!(outcome, Outcome::Usage(None));
        let (outcome, _) = run(&["-t0", "http://host/"]);
        assert_eq!(outcome, Outcome::Usage(None));
    }

    #[test]
    fn reports_invalid_urls() {
        let (outcome, _) = run(&["not-a-url"]);
        assert_eq!(
            outcome,
            Outcome::Usage(Some("invalid URL: not-a-url".to_owned()))
        );
    }

    #[test]
    fn rejects_fewer_connections_than_threads() {
        let (outcome, _) = run(&["-t", "4", "-c", "2", "http://host/"]);
        assert_eq!(
            outcome,
            Outcome::Usage(Some("number of connections must be >= threads".to_owned()))
        );
    }

    #[test]
    fn missing_urls_fall_back_to_usage() {
        let (outcome, _) = run(&["-t2"]);
        assert_eq!(outcome, Outcome::Usage(None));
    }

    #[test]
    fn lists_engines() {
        let (outcome, _) = run(&["-E"]);
        assert_eq!(outcome, Outcome::ListEngines);
    }

    #[test]
    fn getopt_errors_surface() {
        let (outcome, _) = run(&["-z"]);
        assert_eq!(
            outcome,
            Outcome::Usage(Some("wrkrs: invalid option -- 'z'".to_owned()))
        );
    }

    #[test]
    fn usage_lines_pad_to_the_box_width() {
        let mut out = Cursor::new(Vec::new());
        super::print_usage(&mut out);
        let text = String::from_utf8(out.into_inner()).expect("ascii");
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let expected = if index + 1 == lines.len() {
                // The final C literal pads one column short.
                53
            } else {
                54
            };
            assert_eq!(line.chars().count(), expected, "{line:?}");
        }
    }

    #[test]
    fn output_mode_defaults_to_legacy() {
        let (outcome, _) = run(&["http://host/"]);
        let Outcome::Run(config) = outcome else {
            panic!("expected a run");
        };
        assert_eq!(config.output, OutputMode::Legacy);
        assert!(config.output_file.is_none());
    }

    #[test]
    fn parses_the_output_flags() {
        let cases = [
            (&["--output", "json", "http://host/"][..], OutputMode::Json),
            (&["-o", "modern", "http://host/"][..], OutputMode::Modern),
            // Attached values work. Every abbreviation of --output is
            // ambiguous with --output-file, so only the full name.
            (&["--output=modern", "http://host/"][..], OutputMode::Modern),
        ];
        for (list, expected) in cases {
            let (outcome, _) = run(list);
            let Outcome::Run(config) = outcome else {
                panic!("expected a run for {list:?}");
            };
            assert_eq!(config.output, expected);
        }

        for list in [
            &["-O", "report.txt", "http://host/"][..],
            &["--output-file=report.json", "http://host/"][..],
            // The unambiguous prefix of --output-file.
            &["--output-f", "report.json", "http://host/"][..],
        ] {
            let (outcome, _) = run(list);
            let Outcome::Run(config) = outcome else {
                panic!("expected a run for {list:?}");
            };
            assert!(config.output_file.is_some(), "{list:?}");
        }
    }

    #[test]
    fn output_abbreviations_stay_ambiguous() {
        let (outcome, _) = run(&["--outpu", "json", "http://host/"]);
        assert_eq!(
            outcome,
            Outcome::Usage(Some(
                "wrkrs: option '--outpu' is ambiguous; possibilities: '--output' '--output-file'"
                    .to_owned()
            ))
        );
    }

    #[test]
    fn rejects_unknown_output_modes() {
        let (outcome, _) = run(&["-o", "xml", "http://host/"]);
        assert_eq!(
            outcome,
            Outcome::Usage(Some(
                "invalid output mode: xml (expected legacy, modern, or json)".to_owned()
            ))
        );
    }
}
