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

/// Validates the engine selection and reports the runner status.
fn run(config: Box<cli::Config>) {
    let script = config.script.as_deref().and_then(|path| path.to_str());
    match engines::select(script, config.engine.as_deref()) {
        Ok(_) => {
            eprintln!("wrkrs: the benchmark runner lands in phase C3");
            process::exit(1);
        }
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    }
}
