# wrkrs-engine

The scripting engine contract for
[wrkrs](https://github.com/wrkrs/wrkrs), the Rust port of wrk.

A scripting engine plugs a guest language into the benchmark: request
generation, response callbacks, delays, and the done phase. The crate
defines the object-safe `ScriptEngine` trait, the restricted `Value`
type copied across engine contexts, `Capabilities` gating the fast
paths, and the read-only `StatsView` the done phase reads.

wrkrs ships LuaJIT (default), Lua 5.4, and QuickJS implementations.
Third parties implement this contract and ship their own binary; the
`wrkrs-engine-tests` conformance suite validates an implementation
against the same behaviors the bundled engines pass.

See
[ENGINES.md](https://github.com/emrecancorapci/wrkrs/blob/main/ENGINES.md)
for the full guide.
