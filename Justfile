# wrkrs development tasks.
# The legacy C implementation stays buildable through build-c until parity.

default:
    @just --list

# Build every crate in release mode.
# Extra cargo flags can be passed for alternate feature sets.
build features='':
    cargo build --release --locked {{ features }}

# Build with the Lua 5.4 engine instead of the default LuaJIT.
build-lua54:
    just build "--no-default-features --features engine-lua54,engine-quickjs"

# Run the Rust test suite for the whole workspace.
# The second invocation covers the test only stub engine feature.
test features='':
    cargo test --workspace --locked {{ features }}
    cargo test --workspace --locked {{ features }} --features wrkrs/engine-stub

# Check formatting and lints for a chosen feature set.
# The second clippy pass covers the test only stub engine feature.
lint features='':
    cargo fmt --check
    cargo clippy --workspace --all-targets --locked {{ features }} -- -D warnings
    cargo clippy --workspace --all-targets --locked {{ features }} --features wrkrs/engine-stub -- -D warnings

# Apply rustfmt to the whole workspace.
fmt:
    cargo fmt

# Build the legacy C wrk binary (reference build during migration).
build-c:
    make

# Benchmark a URL with the legacy C wrk for reference numbers.
bench url: build-c
    ./wrk -t2 -c100 -d10s {{ url }}
