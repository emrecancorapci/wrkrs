//! wrkrs core: configuration, engine registry, and the benchmark runtime.

pub mod backend;
pub mod cli;
pub mod connection;
pub mod engines;
pub mod eventloop;
pub mod getopt;
pub mod legacy;
pub mod modern;
pub mod parser;
pub mod report;
pub mod resolve;
pub mod runner;
pub mod signals;
pub mod stats;
pub mod tls;
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
