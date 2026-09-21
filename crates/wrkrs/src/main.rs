use std::io;
use std::process;

use wrkrs::cli::{self, Outcome};
use wrkrs::engines;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse("wrkrs", &args, &mut io::stdout()) {
        Outcome::ListEngines => {
            print!("{}", engines::engine_listing(engines::engines()));
        }
        Outcome::Usage(message) => {
            if let Some(message) = message {
                eprintln!("{message}");
            }
            cli::print_usage(&mut io::stdout());
            process::exit(1);
        }
        Outcome::Run(config) => run(config),
    }
}

/// Prepares the run and reports the loop status.
fn run(config: Box<cli::Config>) {
    let script = config.script.as_deref().and_then(|path| path.to_str());
    let selection = match engines::select(script, config.engine.as_deref()) {
        Ok(selection) => selection,
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    };
    // A run without a script still uses the default engine, wrk
    // always carries its Lua environment.
    let entry = match selection.or_else(engines::default_engine) {
        Some(entry) => entry,
        None => {
            eprintln!("wrkrs: no scripting engine compiled in");
            process::exit(1);
        }
    };

    match wrkrs::runner::prepare(&config, entry) {
        Ok(prepared) => {
            let reporter: Box<dyn wrkrs::report::Reporter> = match config.output {
                wrkrs::cli::OutputMode::Legacy => Box::new(wrkrs::legacy::LegacyReporter {
                    latency: config.latency,
                }),
                wrkrs::cli::OutputMode::Modern => Box::new(wrkrs::modern::ModernReporter),
                wrkrs::cli::OutputMode::Json => Box::new(wrkrs::json::JsonReporter),
            };
            // The banner is legacy output: live on stdout before the
            // run, into the file when the report lands there.
            if config.output == wrkrs::cli::OutputMode::Legacy && config.output_file.is_none() {
                let mut out = io::stdout().lock();
                wrkrs::legacy::banner(
                    &mut out,
                    config.duration_s,
                    &config.url,
                    config.threads,
                    config.connections,
                )
                .expect("stdout writes");
            }
            let (report, mut main) = wrkrs::runner::execute(&config, prepared);
            match &config.output_file {
                None => reporter.report(&mut io::stdout().lock(), &report),
                Some(path) => {
                    match std::fs::File::create(path) {
                        Ok(mut file) => {
                            if config.output == wrkrs::cli::OutputMode::Legacy {
                                wrkrs::legacy::banner(
                                    &mut file,
                                    config.duration_s,
                                    &config.url,
                                    config.threads,
                                    config.connections,
                                )
                                .expect("file writes");
                            }
                            reporter.report(&mut file, &report);
                        }
                        Err(error) => {
                            eprintln!("wrkrs: cannot open {path:?} for writing: {error}");
                            process::exit(1);
                        }
                    }
                    eprintln!("wrkrs: report written to {}", path.display());
                }
            }
            report.call_done(main.as_mut());
        }
        Err(error) => {
            eprintln!("{}", error.message());
            process::exit(1);
        }
    }
}
