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
            let (result, mut main) = wrkrs::runner::execute(&config, prepared);
            result.call_done(main.as_mut());
            // The byte exact reporter lands in phase C4.
            eprintln!(
                "wrkrs: {} requests in {:.2}s, {} errors",
                result.complete,
                result.duration_us as f64 / 1_000_000.0,
                result.errors.connect
                    + result.errors.read
                    + result.errors.write
                    + result.errors.timeout
            );
        }
        Err(error) => {
            eprintln!("{}", error.message());
            process::exit(1);
        }
    }
}
