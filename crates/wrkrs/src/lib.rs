//! wrkrs core: configuration, engine registry, and the benchmark runtime.

pub mod backend;
pub mod cli;
pub mod engines;
pub mod getopt;
pub mod parser;
pub mod stats;
pub mod units;

#[cfg(all(feature = "engine-luajit", feature = "engine-lua54"))]
compile_error!(
    "engine-luajit and engine-lua54 are mutually exclusive: \
     mlua links exactly one Lua runtime per binary"
);

#[cfg(not(any(
    feature = "engine-luajit",
    feature = "engine-lua54",
    feature = "engine-quickjs",
    feature = "engine-stub"
)))]
compile_error!("no scripting engine compiled in: enable an engine-* feature");
