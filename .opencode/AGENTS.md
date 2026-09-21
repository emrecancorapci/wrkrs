# AGENTS.md

Guidance for AI agents working in this repository. Also look into `.opencode/RULES.md` and `PLAN.md`.

## Mission

`wrkrs` is the Rust port of `wrk` — a multithreaded, event-driven HTTP
benchmarking tool — preserving observable behavior: CLI, output format,
scripting API, and benchmark semantics. The port reached parity and shipped
as 1.0. The original C implementation was removed at the 1.0 release; it
lives in the git history (`git log --all -- src/`) and the golden fixtures
under `tools/` still pin its observable output.

Scripting is exposed through a **pluggable engine abstraction**: LuaJIT is
the default engine, with Lua 5.4 and QuickJS (JavaScript) available, and
third parties can implement their own engines against the published
contract crate `wrkrs-engine`.

## Ported behavior (the parity spec)

### Runtime architecture

1. **Setup (main thread)**
   - Parse args (`getopt_long`): `-t` threads (default 2), `-c` connections
     (default 10), `-d` duration (default 10s), `-s` script, `-H` header
     (repeatable, requires `": "` separator), `-T`/`--timeout` (default 2s),
     `--latency`, `-v`.
   - Numeric args accept SI units; time args accept `s`/`m`/`h` (`scan_metric`,
     `scan_time`).
   - URL parsed by `http_parser_parse_url`; requires schema + host. `https`
     swaps the socket vtable to the SSL implementation and calls `ssl_init()`.
   - Main Lua state runs setup phase: `wrk.resolve()` → `wrk.lookup()`
     (getaddrinfo, exit on failure) filtered by `wrk.connect()` probes.
   - One **Lua state per thread** is created; `setup(thread)` is called in the
     main state for each; `init(args)` is called in each thread's own state
     (script args come after `--` on the command line).
   - Thread 0 probes the script: `pipeline` = number of requests in the
     generated request string (`script_verify_request`, exits with line:column
     diagnostic on malformed request), `dynamic` = script defines global
     `request()`, `delay` = defines `delay()`, `want_response` = defines
     `response()` (enables header/body buffering into nul-separated buffers).

2. **Run (N threads, N event loops)**
   - Each thread: `connections/threads` connections, non-blocking `connect()`
     - `TCP_NODELAY`, registered `AE_READABLE|AE_WRITABLE` with
     `socket_connected` callback.
   - Static scripts: one request buffer shared by all connections. Dynamic:
     `request()` called per send.
   - Write path (`socket_writeable`): optional `delay()` → time event →
     write loop until done, then drop writable interest.
   - Read path (`socket_readable`): read 8 KiB (`RECVBUF`), feed
     `http_parser_execute`, drain while `n == RECVBUF && readable() > 0`
     (`ioctl(FIONREAD)` / `SSL_pending`).
   - `response_complete`: counts request, status > 399 → error, calls
     `response(status, headers, body)` if requested, records latency
     (`now - c->start`) once `pending` hits 0 (pipeline), re-arms writable,
     reconnects if `Connection: close` / no keep-alive.
   - Time event every 100 ms (`RECORD_INTERVAL_MS`): records per-thread
     req/s into the requests histogram, checks the global `stop` flag
     (SIGINT handler sets it) and stops the loop.
   - Main thread `sleep(duration)`, sets stop, joins threads.

3. **Teardown / reporting**
   - Coordinated omission correction: `stats_correct(latency,
     runtime_us / (complete/connections))`.
   - Output (exact format, see README):
     - `Running <time> test @ <url>` / `<N> threads and <M> connections`
     - `Thread Stats` table: Latency + Req/Sec rows with Avg/Stdev/Max/+/- Stdev
     - `--latency`: 50/75/90/99 percentile distribution
     - totals: requests, bytes (`17.76GB` binary units), socket errors
       (connect/read/write/timeout), non-2xx/3xx count, `Requests/sec`,
       `Transfer/sec`.
   - `done(summary, latency, requests)` called in main Lua state if defined.

### Statistics structure (behavioral-critical)

- `stats` = fixed-size `uint64_t` array of counts indexed by value (µs for
  latency), `limit = max + 1`, lock-free: `__sync_fetch_and_add` on buckets,
  CAS loops for min/max. Latency hist limit = `timeout_ms * 1000`; requests
  hist limit = `MAX_THREAD_RATE_S` (10,000,000).
- `stats_record` returns 0 for out-of-range values → counted as timeout.
- `stats_correct`, `stats_percentile` (rank = `round(p/100 * count + 0.5)`),
  `stats_value_at` (Lua `latency(i)` call), `stats_popcount` (`#latency`).

### Lua API (must remain compatible — see `SCRIPTING` + `scripts/`)

- Global `wrk` table: `scheme`, `host`, `port`, `method`, `path`, `headers`,
  `body`, `thread`; functions `wrk.format([method, path, headers, body])`,
  `wrk.lookup(host, service)`, `wrk.connect(addr)`.
- Optional globals: `setup(thread)`, `init(args)`, `delay()`, `request()`,
  `response(status, headers, body)`, `done(summary, latency, requests)`.
- `thread` userdata: `.addr` (get/set), `:get(name)`, `:set(name, value)`,
  `:stop()`; values copied across Lua VMs are restricted to
  boolean/nil/number/string/tables-of-same.
- `latency`/`requests` userdata: `.min .max .mean .stdev`, `:percentile(p)`,
  callable `latency(i)` → value+count, `#obj` → popcount.
- Default request built by `wrk.format()`: `GET <path> HTTP/1.1`, headers from
  `wrk.headers` (auto `Host`, `Content-Length` only when body present),
  `\r\n` joined.

## Implementation

The actual crate layout (the migration scaffolding is history now):

```md
├─Cargo.toml             # workspace root
├─crates/
│ ├─wrkrs-engine/        # publishable contract crate: ScriptEngine trait,
│ │                      # Value, Capabilities, ThreadApi, StatsView
│ ├─wrkrs-engine-tests/  # engine-agnostic conformance suite (private)
│ └─wrkrs/               # binary + core
│   └─src/
│     ├─main.rs          # CLI wiring and orchestration
│     ├─cli.rs           # getopt port, Config, usage
│     ├─engines/         # compile-time registry: lua (luajit + lua54,
│     │                  # with the default wrk.lua environment) and quickjs
│     ├─connection.rs    # connection state machine over the Socket seam
│     ├─eventloop.rs     # mio loop, timers, rate tick
│     ├─parser.rs        # httparse glue: URL parsing, request
│     │                  # verification, response framing
│     ├─stats.rs         # histogram (stats.c port)
│     ├─legacy.rs        # byte compatible reporter
│     ├─modern.rs        # clean v1 reporter
│     ├─json.rs          # machine readable v1 reporter
│     ├─report.rs        # RunReport + Reporter trait
│     ├─tls.rs           # rustls transport
│     ├─units.rs         # scan and format units (units.c port)
│     └─signals.rs       # SIGINT and SIGPIPE
├─scripts/               # example scripts, Lua and JavaScript
└─tools/                 # golden fixtures and live harnesses
```

Cargo features: `engine-luajit` (default) + `engine-quickjs` (default);
`engine-lua54` is mutually exclusive with `engine-luajit` (mlua links exactly
one Lua runtime per binary). `engine-stub` exists for test-only builds.

### Scripting engine strategy (decided)

- **Default engine: LuaJIT** via `mlua` (`luajit` + `vendored` features).
- **Additional engines:** `lua54` (via `mlua` `lua54` + `vendored`) and
  `quickjs` (JavaScript, via `rquickjs`).
- **Flags:** `-e, --engine <name>` selects the engine explicitly;
  `-E, --engines` lists the engines compiled into the binary and exits.
  Without `-e`, the engine is chosen by script file extension (`.lua` → the
  Lua engine in the build, `.js` → quickjs); unknown extension without `-e`
  is an error listing available engines.
- **Registration is compile-time only** (v1): a cfg-gated static registry in
  the `wrk` crate. No dylib/`--engine-path` plugins, no WASM, no Rhai in v1 —
  the trait keeps those additive for later.
- **Contract:** object-safe, `Send` `ScriptEngine` trait in the publishable
  `wrkrs-engine` crate, with `Value` (the restricted cross-VM copy type:
  null/bool/int/float/string/nested tables), `Capabilities` (replaces the C
  probe functions `script_is_static`/`want_response`/`has_delay`/`has_done`,
  gating the same fast paths), `ThreadApi` (`addr` get/set, `get`/`set`/`stop`),
  and a read-only `StatsView` (min/max/mean/stdev/percentile/value_at/popcount)
  for `done()`.
- **Shared logic lives in core, once:** request formatting (`wrk.format`
  semantics incl. Host/IPv6/Content-Length rules), DNS lookup/connect probing,
  pipeline detection (core parses the generated request with httparse — same
  as `script_verify_request`). Engines bind these idiomatically in the guest
  language; they do not reimplement them.
- **Error semantics match C:** a script that fails to load prints
  `<file>: <error>` to stderr and the run continues with default behavior
  (script.c prints and does not exit). Hot-path errors are fatal for
  parity in v1; a recoverable error counter is a possible documented
  deviation later.
- **Conformance suite:** `wrkrs-engine-tests` runs every engine against the
  scripting contract (ports of `scripts/*` cases, `Value` copy restrictions,
  capability honesty, error propagation). An engine that passes is considered
  compatible; the suite is also the LuaJIT regression net.
- **Third-party story:** implement `ScriptEngine` against `wrkrs-engine`,
  register it on the runner, ship your own binary. Documented in `ENGINES.md`.
- **Known trade-offs:** QuickJS is an interpreter — per-request callbacks cap
  throughput vs LuaJIT (document for users); `-e lua54` requires a build with
  `--features engine-lua54` (clear error message says so).

### Output modes (decided)

- One `RunReport` data model (run config, summary, error counts, latency and
  rate stats through `StatsView`) rendered by a `Reporter` trait. Every mode
  reads the same histogram, only formatting differs — modes never show
  different numbers.
- Modes for v1: `legacy` (byte-exact C output, the quirk list below is its
  spec), `modern` (clean human layout without the unit-ladder quirks), and
  `json` (one machine-readable object). An `html` report is v2 and can
  render from the JSON shape.
- Flags: `--output <legacy|modern|json>` and `--output-file <path>`, short
  forms `-o` and `-O`. Every abbreviation of `--output` is ambiguous with
  `--output-file`, only the full name works, `--output-f` prefixes
  `--output-file`.
- The banner is legacy output: live on stdout before the run, and into the
  file when the report lands in one. Other modes print no banner. A file
  destination leaves stdout clean, one note goes to stderr.
- Defaults: `legacy` is the default for 1.0 so wrkrs stays a drop-in
  replacement, the default flips to `modern` at 2.0.
- The v1 `modern` mode is a static report. Live progress (countdown and
  rolling rate on the 100 ms tick when stdout is a TTY) is a follow-up.

### Parity requirements (acceptance criteria)

1. `--help`, `-v` output equivalent.
2. The `legacy` output mode is byte-compatible with the README example
   (spacing, units, `+/- Stdev`, percentile table under `--latency`), and
   `legacy` is the default mode for 1.0 (see "Output modes").
3. All 9 scripts in `scripts/` run unmodified and behave identically
   (incl. `report.lua` histogram output, `pipeline.lua`, `stop.lua`,
   `delay.lua`, `setup.lua` cross-thread `get`/`set`).
4. Defaults: threads=2, connections=10, duration=10s, timeout=2s;
   pipeline=1 unless script pipelins; connections ≥ threads enforced.
5. Error counters (connect/read/write/timeout/status) and CO correction
   preserved.
6. `https://` targets work with SNI, cert verification disabled (as today).
7. SIGINT stops gracefully and still prints the report; SIGPIPE ignored.
8. Scripting engines: `-e/--engine` selects an engine, `-E/--engines` lists the
   ones compiled in; `.lua` scripts dispatch to the built-in Lua engine with no
   flag. All engine additions must not change behavior of existing Lua scripts
   under the default (LuaJIT) engine. New `scripts/*.js` equivalents pass the
   same conformance suite.

### Known behavioral quirks to preserve (or consciously drop)

- The benchmark URL is the first entry of the script arguments, so
  `init` sees the URL at `args[0]` and user arguments from `args[1]`
  (wrk passes `argv[optind..]` untrimmed).
- `print_units` pads by unit-suffix width (the pad shrinks when the last
  one or two characters are letters) and its `%*.*s` precision TRUNCATES
  over-long strings — output spacing is load-bearing for users parsing
  output.
- Unit promotion happens at 85% of the next unit (`scale * 0.85`), so
  850 µs prints as `0.85ms`. The µs ladder stops at `ms` (its units list
  carries no seconds step), so 999 999 µs prints as `1000.00ms` — only
  values of at least 1 000 000 µs take the seconds pre-conversion in
  `format_time_us`.
- Totals pad through `%9.2Lf` (Requests/sec) and `%10sB` (Transfer/sec),
  and the `-v` line carries a trailing space after the event loop name.
- C formats through `long double`, which is 80-bit on x86-64 but plain
  f64 on ARM, so C wrk itself differs across platforms. Rust uses f64
  everywhere (documented deviation, matches the C output on ARM; rare
  last-digit differences are possible on x86-64).
- wrkrs uses rustls where C uses OpenSSL: verification stays off and
  SNI carries the host, but rustls declines the legacy TLS 1.0/1.1 and
  weak cipher negotiations an `SSLv23_client_method` would accept, so
  ancient servers may refuse wrkrs where C succeeds (documented
  deviation).
- The usage box pads every line to 54 columns and its final line pads
  one short (53), wrkrs reproduces both down to the byte.
- A stop can strand one or two in flight requests between the thread
  counters `request()` and `response()` maintain; the C binary shows
  the same gaps in scripts/setup.lua output.
- `scan_units` accepts units case-insensitively, max 2 chars (`1K` == `1k`).
- Timeout is recorded only when a response actually arrives too late
  (`stats_record` range failure), and latency cap = timeout histogram limit.
- Host header IPv6 bracketing + port appending rules in `wrk.init()`.
- Rate is sampled every 100 ms per thread (`Req/Sec` = per-thread rates).
- Response headers passed to Lua as nul-separated pairs (lossy for headers
  containing nul — unlikely, but format is what scripts see).
- `script_request` reuses/reallocs one buffer per connection (`realloc` growth
  pattern irrelevant, but request bytes must be the Lua string verbatim).
- Connections are eagerly (re)connected on error — no backoff, matching
  original reconnect storm behavior.

### Licensing / attribution

wrk is Apache 2.0 (see `LICENSE`, `NOTICE`). Vendored components carry their
own notices (redis ae — BSD; joyent http-parser — MIT; LuaJIT — MIT; OpenSSL —
Apache-2.0). The Rust port should keep `NOTICE` attribution for derived
designs (ae→mio usage may drop vendored code; httparse is a rewrite of
http-parser). If rustls replaces OpenSSL, the README "Cryptography Notice"
export-control section can be removed — flag for maintainer decision.

## Working conventions

- Rust: edition 2024, `#![deny(unsafe_op_in_unsafe_fn)]` where unsafe is
  required (mlua/rquickjs FFI); prefer safe code outside FFI boundaries.
- Formatting: `cargo fmt`; lints: `cargo clippy -- -D warnings`.
- Engine feature combos to keep green:
  `cargo build` (luajit + quickjs) and
  `cargo build --no-default-features --features engine-lua54,engine-quickjs`.
  `engine-luajit` + `engine-lua54` together must fail with `compile_error!`.
- Verify changes with: `cargo build --release` and a quick local benchmark,
  e.g. `./target/release/wrkrs -t2 -c10 -d2s http://127.0.0.1:8080/` against a
  local server. The tools/ harnesses compare against a C wrk binary when one
  is present and run their wrkrs assertions otherwise.
- When touching the engine layer, run the conformance suite for every engine
  your change affects (`just test` runs the default and stub legs, add
  `just test "--no-default-features --features engine-lua54,engine-quickjs"`
  for the Lua 5.4 leg; `--all-features` is impossible because the Lua
  engines are mutually exclusive).
- When editing this file, keep the parity requirements and quirks sections
  accurate — they are the source of truth for migration decisions.

## Additional Changes

- Build and test actions run through `just`, the C `make` build was removed
  at 1.0 with the C tree.
- Dependencies stay at their latest stable versions where possible.
- TLS is rustls (see the documented quirk about the TLS floors).
- This guidance directory ships with the repository.
