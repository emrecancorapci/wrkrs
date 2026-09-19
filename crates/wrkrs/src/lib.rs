//! wrkrs core: configuration, engine registry, and the benchmark runtime.

#[cfg(all(feature = "engine-luajit", feature = "engine-lua54"))]
compile_error!(
    "engine-luajit and engine-lua54 are mutually exclusive: \
     mlua links exactly one Lua runtime per binary"
);
