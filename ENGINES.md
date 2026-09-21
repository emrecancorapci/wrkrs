# Scripting Engines

wrkrs drives benchmark scripts through pluggable scripting engines. Lua
is the classic surface, JavaScript is available out of the box, and the
`wrkrs-engine` contract lets other languages plug in.

## Selecting an engine

Without flags the engine is chosen by the script file extension:

```bash
wrkrs -t2 -c10 -d10s -s bench.lua http://127.0.0.1:8080/   # Lua engine
wrkrs -t2 -c10 -d10s -s bench.js  http://127.0.0.1:8080/   # QuickJS
```

`-e, --engine <name>` selects an engine explicitly and overrides the
extension, `-E, --engines` lists the engines compiled into the binary:

```bash
$ wrkrs -E
  luajit     .lua    LuaJIT 2.1 (vendored)
  quickjs    .js     QuickJS (rquickjs)
```

Failure cases carry fixed messages:

| Situation | Message |
| --- | --- |
| `-e` without `-s` | `option -e requires a script (-s)` |
| Known engine not compiled in | `engine 'lua54' not compiled in, rebuild with --features engine-lua54` |
| Unknown engine name | `engine 'x' not compiled in, compiled engines: ...` |
| No engine handles the extension | `no engine handles the 'txt' extension, compiled engines: ...` |
| Script without an extension | `'bench' has no extension, compiled engines: ...` |

## Compiled-in engines

| Name | Extension | Runtime | Build |
| --- | --- | --- | --- |
| `luajit` | `.lua` | LuaJIT 2.1, vendored | default features |
| `lua54` | `.lua` | Lua 5.4, vendored | `--no-default-features --features engine-lua54,engine-quickjs` |
| `quickjs` | `.js` | QuickJS via rquickjs | default features |

Engines register at compile time through cargo features, so the binary
always knows exactly what it runs and script dispatch never guesses.

`engine-luajit` and `engine-lua54` are mutually exclusive: mlua links one
Lua runtime per binary and the runtimes export clashing symbols. Building
with both fails with a `compile_error!` that says so. Both Lua builds run
the same engine implementation and the same conformance suite, and `.lua`
dispatches to whichever Lua runtime the build carries.

## Lua engines

The `SCRIPTING` document stays normative for the Lua API and every script
in `scripts/*.lua` runs unmodified. The default environment is embedded
verbatim from `crates/wrkrs/src/engines/lua/wrk.lua`, so table
iteration order and the
`wrk.format` mutation quirks are identical to wrk.

Preserved quirks worth knowing:

- Script arguments arrive in `args[0]`, `args[1]`, ... (wrk fills the
  table from index zero).
- A script that fails to load prints `<file>: <error>` on stderr and the
  run continues with the default request.
- Header order in generated requests follows Lua table hashing, exactly
  like wrk. Scripts that need a fixed order build the header list
  themselves.
- Both Lua builds map numbers through the same value transfer rules as
  wrk's `script_copy_value`.

## JavaScript engine

QuickJS implements the same surface with JavaScript shapes. The examples
in `scripts/*.js` cover the same cases as the Lua examples.

The `wrk` object carries `scheme`, `host`, `port`, `method`, `path`,
`headers`, `body`, and `thread`, plus `wrk.format`, `wrk.lookup`, and
`wrk.connect`. The optional globals are `setup(thread)`, `init(args)`,
`delay()`, `request()`, `response(status, headers, body)`, and
`done(summary, latency, requests)`. The thread object exposes `addr`
(read and write), `get(name)`, `set(name, value)`, and `stop()`. The
stats objects expose `min`, `max`, `mean`, `stdev`, `length`, the
`percentile(p)` method, and `call(i)` returning `[value, count]` — the
JavaScript shape of the Lua call operator. A `print` function writes a
line to stdout.

Differences from the Lua engines:

- Assigning an undeclared global throws. Declare script state with
  `var` or `let` at the top level.
- Response bodies arrive as strings. Bytes that are not valid UTF-8
  become replacement characters.
- Header order in generated requests follows JavaScript property
  insertion order.
- Each engine instance caps its heap at 256 MiB and its stack at
  1 MiB, so a runaway script fails its call instead of the process.

## Performance guidance

Script callbacks run on the request path. The cheapest script is a
static one: build the request once in `init` and return it from
`request`, which lets the runner cache the bytes. Building a fresh
request per call costs allocation and formatting time on every request.

LuaJIT executes callbacks in well under a microsecond. QuickJS is an
interpreter and pays several times that per callback, so per-request
work caps throughput noticeably at high request rates. Prefer static
requests under QuickJS, or keep `request` to a precomputed lookup.

## Implementing an engine

Engines implement the `ScriptEngine` trait from the `wrkrs-engine` crate,
which also carries the host services an engine consumes:

- `ScriptSpec` and `UrlRef` describe one scripting environment
- `ResolveApi` backs `wrk.lookup` and `wrk.connect`
- `ThreadApi` backs the thread object (`addr`, `get`, `set`, `stop`)
- `StatsView` backs the stats objects handed to `done`
- `Value` is the restricted cross-environment copy type
- `Capabilities` tells the runner what the script needs

A minimal engine looks like this:

```rust
use wrkrs_engine::{EngineError, ScriptEngine, ScriptSpec};

pub struct MyEngine { /* the VM */ }

impl ScriptEngine for MyEngine {
    fn create(spec: &ScriptSpec) -> Result<Self, EngineError>
    where Self: Sized {
        // build the VM, install the wrk table, run the script file
        todo!("build the environment")
    }

    // resolve, setup, init, delay, request, response, done,
    // capabilities, get_global, set_global follow
}
```

Engines must be `Send` because the runner moves one environment per
thread into its worker. Host services arrive as `Arc` handles since
scripts keep the objects past the callback that delivered them.

The conformance suite in `wrkrs-engine-tests` drives any engine through
the observable contract: request bytes, callback wiring, value transfer
rules, capability honesty, and error propagation. Point the suite at
your factory with your language's script sources and a green run is the
compatibility bar.

Registration is compile time in v1: add an `EngineEntry` to the
registry table with the name, extensions, description, and factory, then
ship your own binary. Runtime loading is deliberately out of scope for
now, the trait keeps it additive.

```rust
// crates/wrkrs/src/engines/mod.rs
pub fn engines() -> &'static [EngineEntry] {
    &[
        // ...
        EngineEntry {
            name: "mylang",
            extensions: &["my"],
            description: "My language engine",
            factory: mylang::factory,
        },
    ]
}
```
