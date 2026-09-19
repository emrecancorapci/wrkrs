use std::process;

use wrkrs::cli;
use wrkrs::engines;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags = match cli::scan(&args) {
        Ok(flags) => flags,
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    };

    if flags.list_engines {
        print!("{}", cli::engine_listing(engines::engines()));
        return;
    }

    match engines::select(flags.script.as_deref(), flags.engine.as_deref()) {
        Ok(Some(entry)) => {
            println!("wrkrs {} [{}]", env!("CARGO_PKG_VERSION"), entry.name);
        }
        Ok(None) => {
            println!("wrkrs {}", env!("CARGO_PKG_VERSION"));
        }
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    }
}
