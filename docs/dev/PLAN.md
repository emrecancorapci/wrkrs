# wrkrs 2.0 Plan

The 2.0 line, ordered by execution rather than by number. `TODO.md` holds the candidate
descriptions, this file holds the execution plan.

Ground rules: small commits, every commit builds and passes
`just lint` and `just test` standalone, both engine legs stay green,
incomplete commits are repaired with fixup and autosquash. Nothing
is pushed without review.

## Order of work

Numeric order is no longer execution order. V1-V4 are done. The
2.0 flip is gated on V9 alone: V9 versions the report that
machine consumers parse, while the modern text layout is a human
format that carries no stability guarantee, so the later phases
may add rows to it after 2.0 without a semver break. The rate
mode moves behind the flip as a 2.x feature. From there the line
runs:

1. V9 the report schema policy, small and first, it guards every
   report shape change below
2. V5 modern becomes the default, the 2.0 bump, gated on V9 only
   (see the note in the phase)
3. V10 constant-rate mode with send-time latency accounting, a
   2.x feature, depends on V9
4. V11 load-generator self-diagnostics, opt-in, additive-only
   under the V9 schema policy
5. V12 regression comparison and thresholds, opt-in,
   additive-only under the V9 schema policy
6. V13 the wrk Lua compatibility suite, engine side work that can
   run in parallel once V10 has landed
7. V7 live progress, after V11 so diagnostics warnings can join
   the display (see the note in the phase)
8. V8 the npm package, P2 grows the typed test API and depends on
   V9-V12
9. V14 HTTP/2, a 2.x phase by design, after the 2.0 line settles,
   HTTP/3 an explicitly later phase after it
10. V6 the python engine, deferred, see the note in the phase

The phase numbers stay stable so existing references to them
hold.

## Phase V1 — the config engine

Scripts without scripts: a data file shapes the load. Done, the stop
conditions landed with it because counting responses is the same
machinery the engine already needed, so V2 narrows to templating.

- [x] `crates/wrkrs/src/engines/config/`: parse a TOML or JSON file
      into a request spec through one serde struct
- [x] schema v1: `[request]` with `method`, `path`, `body`, and a
      headers table, plus `[stop]` with `requests` and `bytes`
      (absent sections mean the defaults)
- [x] dispatch on the `.toml` and `.json` extensions through the
      existing engine registry, `-e config` overrides by name
- [x] the engine reports static capabilities for literal requests,
      dynamic once templating lands in V2
- [x] stop conditions wired per thread, `requests` counts completed
      requests and `bytes` counts response body bytes, a stop
      condition turns response parsing on
- [x] `-E` lists the config engine alongside the scripting engines
- [x] acceptance: a `bench.toml` describing a POST runs and reports
      exactly like the equivalent `post.lua`; parse unit tests; the
      script e2e harness grows config file cases

## Phase V2 — templating

- [x] light templating in `path`, `body`, and header values:
      `{{n}}` is a per thread counter starting at one, `{{rand}}` is
      a random integer, substitution happens per burst so a
      templated file reports dynamic capabilities
- [x] acceptance: a templated config file behaves like `counter.lua`
      on the wire

## Phase V3 — capture rules

- [x] `[capture]`: copy a response header from one path onto the
      headers of later requests, the auth.lua flow without a script
- [x] a capture rule turns response buffering on, the wants response
      capability, so the file format stays honest about its cost
- [x] acceptance: the auth flow as a config file passes the same
      e2e assertions as `auth.lua`

## Phase V4 — the html reporter

- [x] `-o html`: render the JSON shape into one self contained page,
      inline styles and scripts, no external assets and no network
- [x] the latency distribution charted, the totals and error blocks
      in place, numbers identical to the JSON mode by construction
- [x] acceptance: golden test on a synthetic report, valid output
      asserted with a parse

## Phase V5 — modern becomes the default

- [ ] flip the default output mode, `-o legacy` unchanged
- [ ] README, AGENTS.md, and the usage text updates
- [ ] the 2.0 version bump rides with this, the semver promise for
      the default flip

> Note (order of work): V9 lands before this flip, it versions
> the report that machine consumers parse. The gate is V9 only,
> the recorded maintainer decision, reversing the earlier V9-V10
> gate: the modern text layout is a human format and carries no
> stability guarantee beyond the legacy-compatible fields, so
> later phases may add rows to it without a semver break, and the
> guarantee for tools is `-o json` under the V9 policy. V10 moves
> after the flip as a 2.x feature, V11 and V12 ship as opt-ins
> and none of them gates 2.0.

## Phase V6 — the python engine

- [ ] feature `engine-python` via pyo3, dispatch on `.py`, the wrk
      surface exposed as a module, `Value` mapped to native Python
      objects
- [ ] conformance suite passes for the new engine
- [ ] document the GIL serialization for dynamic scripts and the
      linking story, vendored or system Python
- [ ] acceptance: `scripts/*.py` equivalents of the example scripts
      pass the e2e assertions

> Note (2.0 planning): deferred. The differentiation against the
> Lua, JavaScript, and config engines is low, the GIL serializes
> dynamic-script callbacks, and the pyo3 linking story, vendored
> or system Python, plus binary size lands on all six release
> legs. Revisit after V14. The phase stays for a later line, do
> not schedule it in 2.0.

## Phase V7 — live progress

- [ ] a countdown and a rolling request rate refreshed on the 100 ms
      tick while a run is in flight, only when stdout is a TTY
- [ ] pipes and file captures stay byte clean, the isatty check
      decides
- [ ] acceptance: captured output is identical with and without a
      TTY

> Note (2.0 planning): if V11 has landed first, decide whether
> diagnostics warnings surface inside the live display, TTY only,
> or stay stderr-only so the display carries pure progress.

## Phase V8 — the npm package

The wrkrs binary inside npm packages with a JavaScript API over it:
`run(options) -> report`, pure JS, no FFI, the binary straight from
the release pipeline. Sequenced after the capability phases; the
detail lives in PLAN-packaging.md, which is the plan for this line.

- [ ] P1 the contract and scaffold: `run()` over `-o json`, tests,
      the `node` CI job, the npm name claimed
- [ ] P2 the full API: config file options, abort, error taxonomy,
      types, the bin shim, and the typed test API,
      `latency_uncorrected`, `latency_send_time`, `diagnostics`,
      and a compare helper with threshold results in
      assertion-friendly types, tested over fixtures from each
      phase (depends on V9-V12)
- [ ] P3 platform packages: the six release legs as
      `optionalDependencies`, resolution, clean failure
- [ ] P4 the publish pipeline: release-published gated, checksum
      verified, provenance
- [ ] P5 docs: the package README and the root Packages section

> Note (2.0 planning): the embeddable test API, a typed
> `run(options) -> report` with runner-friendly assertions such as
> `expect(report.p99).toBeLessThan(50)`, belongs in P2 rather
> than a separate phase. P2 must expose the corrected,
> uncorrected, and send-time latency of V10, the diagnostics of
> V11, and the compare and threshold results of V12, so it
> depends on V9-V12.

## Phase V9 — the report schema policy

Version the machine readable report before anything changes its
shape. Consumers parse `-o json` and the JSON embedded in
`-o html`, and the V8 npm API and the V12 compare command will
too, but the only versionish field today is `version` in
`json.rs`, which carries the wrkrs build that produced the
report, not the shape of the report, so a consumer cannot tell
which fields a document is allowed to carry.

- [ ] `json.rs`: a `schema` field on `JsonReport` behind a
      `SCHEMA` const, debuting at 2, the historical object
      implicitly 1, absent means 1 on the parse side
- [ ] `html.rs`: the embedded document carries `schema`, the
      golden fixture updated
- [ ] the additive-only policy written where contributors read
      it, AGENTS.md and the `-o json` blurb in README: fields are
      added, never renamed, retyped, or removed, consumers ignore
      unknown fields, only a non-additive change bumps `schema`
- [ ] name the fields the later phases add so the contract is
      settled before they land: V10 `mode`,
      `latency_uncorrected`, and `latency_send_time`, V11
      `diagnostics`, V12 a compare document with its own `schema`
- [ ] PLAN-packaging.md: the report field list gains `schema`, the
      distinction from `version` stated
- [ ] acceptance: a golden test parses an old report fixture
      ignoring unknown fields, and a fresh report carries
      `schema: 2` with every existing field unchanged

  Notes: effort S. `-o legacy` never carries schema, no wrk
  surface moves. Open question: a constant schema number between
  breaks (proposed) vs bumping on every addition.

## Phase V10 — constant-rate mode

Fixed-rate load with send-time latency accounting. The framing
matters: wrk does correct for coordinated omission, approximately
and after the fact, and the port keeps that byte for byte, so the
goal is not correction versus none but true open-loop measurement
beside wrk's average-interval approximation. wrkrs is closed loop
only today: a connection sends its next burst when the previous
one drains (`connection.rs` re-arms writable in `rearm_or_close`),
the correction in `runner.rs` mutates the only histogram in place
so no output mode or `done()` callback can see the uncorrected
numbers, and nothing can hold a request rate the way wrk2 does.
A rate run reports three numbers, wrk-style corrected,
uncorrected, and send-time: `latency` keeps its wrk-compatible
corrected meaning in every mode, `latency_uncorrected` carries
the raw histogram, and the new `latency_send_time` field carries
the send-time distribution and is the primary lens for rate runs
in modern, json, and html display, a display matter, not a field
replacement; `-o legacy` refuses `-R`. A 2.x feature after the
V5 flip, it depends on V9 only. The design lives in
PLAN-constant-rate.md.

- [ ] snapshot the raw latency histogram in `runner.rs::execute()`
      before `correct()` runs and carry both in `RunReport`, no
      output change yet: every reporter still reads the corrected
      histogram exactly as today, and the golden stats test in
      `stats.rs` drives `Histogram::correct` directly and is
      untouched, so `-o legacy` stays byte-identical
- [ ] `-R, --rate <N>` in `cli.rs`, SI units, validation, the
      usage line, the closed-loop default untouched
- [ ] the open-loop scheduler in `eventloop.rs`: per-thread send
      deadlines on the existing `Deadline` heap, sends decoupled
      from responses, connections opened on demand up to `-c`
- [ ] send-time latency accounting in `connection.rs`: under a
      rate the clock starts at the intended send, not the write
- [ ] the report surfaces behind the V9 policy: `mode`,
      `latency_uncorrected`, and `latency_send_time` in `json.rs`,
      `latency` unchanged in meaning everywhere, the send-time
      distribution primary in `modern.rs` and `html.rs` under
      `-R`; `-R` with `-o legacy` is refused with a clear error,
      decided, see PLAN-constant-rate.md
- [ ] e2e: a fixed-rate run against the local harness asserts the
      request count within tolerance and both percentiles, a
      `tools/` leg compares against a wrk2 binary when one is
      present, the wrk comparison legs stay green for closed loop
- [ ] docs: README options, AGENTS.md notes, the mode naming
      stated unambiguously, the closed loop is wrk-compatible
      with wrk's approximate interval correction, `-R` is the
      open-loop mode with send-time accounting, wrk2 the
      reference
- [ ] acceptance: against a server with an injected stall, `-R
      1000` holds ~1000 req/s while the send-time percentiles
      show the stall and the uncorrected ones do not, and a run
      without `-R` is byte-identical to the current binary

  Notes: effort L, a 2.x phase after the V5 flip, the largest of
  them. Risks and open questions, pipeline scripts under a rate,
  timeout accounting when slow responses pile past the histogram
  limit, live in PLAN-constant-rate.md.

## Phase V11 — load-generator self-diagnostics

Answer "is this the server or my client?" before the user asks.
A saturated load generator measures itself, CPU-bound threads, a
lagging event loop, imbalanced threads, exhausted ephemeral ports
and file descriptors all skew the numbers silently, and the only
hints today are the connect-error counter, where `connect_slot`
folds every failure including `EADDRNOTAVAIL` and `EMFILE`, and
the benchmarking-tips prose in README. The 100 ms rate tick in
`eventloop.rs` is the natural sampling point. The design lives in
PLAN-diagnostics.md.

- [ ] tick instrumentation in `eventloop.rs`: event-loop lag from
      the fixed `rate_deadline` schedule, a busy fraction from
      poll and process timing
- [ ] errno classification in `connect_slot`: `EADDRNOTAVAIL`,
      `EMFILE`, `ENFILE` counted distinctly, still summed into
      `errors.connect` for compatibility
- [ ] per-thread counters preserved in `runner.rs` long enough to
      measure imbalance at the join
- [ ] the thresholds module: sustained-condition rules, warmup
      windows, the false-positive rules written down as contract
- [ ] the warning format: `wrkrs: warning: <code>: <detail>` on
      stderr under the opt-in `--diagnostics` flag, a
      `diagnostics` block in `json.rs` under the V9 policy, a
      warnings block in `html.rs`, stdout stays clean
- [ ] e2e: an undersized client, tiny `-t` against a fast local
      server, trips the cpu or lag warning, a huge `-c` on
      loopback trips the port warning, a healthy run stays silent
- [ ] acceptance: the undersized run warns once per sustained
      condition in the documented format, the healthy run warns
      nothing, and stdout is byte-identical with diagnostics on
      and off

  Notes: effort M. Lands after the V5 flip and never gates it.
  The reporting rides the opt-in `--diagnostics` flag at first;
  enabling stderr warnings by default, stdout still untouched, is
  a later decision once the false-positive rules have proven
  themselves in practice, deliberately not scheduled. Everything
  it adds to the report is additive-only under the V9 schema
  policy. Open question: the hot-path cost of timing, in
  PLAN-diagnostics.md.

## Phase V12 — regression comparison

Make wrkrs usable as a CI gate. A single run is noisy, teams
asking "did this change regress p99" need repeated runs,
aggregation, thresholds, and an exit code the current binary
cannot give them. The design lives in PLAN-compare.md.

- [ ] `wrkrs compare <baseline> <new>` subcommand dispatch in
      `main.rs` ahead of getopt, plain `wrkrs <url>` parsing
      untouched
- [ ] report loading against the V9 `schema` field, single
      objects and NDJSON, unknown schema refused with a clear
      message
- [ ] `--runs N` and `--warmup W` on the run side, repeated
      reports written newline-delimited through `-O`
- [ ] aggregation: per-field medians and a seeded bootstrap
      interval, the statistic per field fixed and documented,
      error rate pooled not medianed
- [ ] a minimum run count guard: compare warns when fewer than 5
      runs are on either side (proposed), bootstrap intervals
      from very few runs mislead
- [ ] thresholds and exit codes: `--threshold 'p99<50ms'`,
      `--threshold 'error_rate<0.1%'`, exit 0 pass, 1 threshold
      failed, 2 usage or input error
- [ ] the compare output: a human table for the terminal, a
      compare document with its own `schema` for `-o json`
- [ ] acceptance: synthetic fixtures with known distributions
      produce deterministic verdicts and exit codes, and a live
      e2e fails a deliberately slowed local server while a
      healthy one passes

  Notes: effort M. Lands on either side of the V5 flip and never
  gates it, the subcommand is invoked explicitly and so is
  inherently opt-in, everything it adds is additive-only under
  the V9 schema policy. Open questions, the interval method,
  median vs interval-bound thresholds, and default warmup, in
  PLAN-compare.md. CI runners are shared machines, the docs must
  say so.

## Phase V13 — the wrk Lua compatibility suite

Prove script compatibility with real scripts, not just the
examples. The conformance suite in `wrkrs-engine-tests` covers
the engine contract, default request, post, pipeline, delay,
response and done capture, setup and init transfer, capabilities,
error paths, and `tools/script_e2e.py` runs the nine bundled
scripts against a local server with a C wrk comparison leg, but a
user asking "will my wrk script run" deserves a tested answer per
API. The phase follows V10 because `done()` receives the latency
and rate histograms, `report.rs` hands them to `call_done`, and
V10 changes what those carry. The curation and the tables live in
PLAN-lua-compat.md.

- [ ] curate the real-world script list, community patterns
      first, per-script license notes, in-repo ports where a
      vendor is not clean
- [ ] the supported-surface table: every `wrk` table field,
      `wrk.format`, `wrk.lookup`, `wrk.connect`, all six
      callbacks, the thread object, both stats objects, each with
      status and notes, published beside the Lua scripting doc
- [ ] the known-differences document, user facing, seeded from
      the AGENTS.md quirks and the engines.md deviations
- [ ] new engine-agnostic cases in `wrkrs-engine-tests` for the
      API edges the examples miss, extending the suite, no
      parallel one
- [ ] a conformance case that `done()` still receives the
      corrected histogram, both in a closed-loop run and under
      `-R`
- [ ] new behavioral cases in `tools/script_e2e.py` with wrk
      comparison assertions where the C binary is present
- [ ] acceptance: every curated script passes e2e on wrkrs and,
      where the harness has the C binary, the observable behavior
      matches, and every supported row of the table has a test
      behind it

  Notes: effort M. Open questions, which community scripts are
  vendorable under their licenses and where the table lives, in
  PLAN-lua-compat.md.

## Phase V14 — HTTP/2

Speak the protocol modern servers negotiate, on request. wrkrs
speaks HTTP/1.1 only: the response framer in `parser.rs` is
httparse based, `tls.rs` builds its rustls config with no ALPN so
https always lands on 1.1, and one TCP connection per `-c`/`-t`
slot carries one in-flight burst with a pipeline depth. TODO.md
already names the shape, an h2 engine is a second framing layer
beside the response framer. HTTP/1.1 stays the default for every
run: wrk is 1.1 only, and silently moving existing https users
onto h2 would cut their results off from their own history, so h2
is requested with an explicit `--http2`. This is a 2.x phase,
after the 2.0 line, HTTP/3 an explicitly later phase after it.
The design lives in PLAN-http2.md.

- [ ] `--http2` in `cli.rs`, long-only with no short form: `-H`
      is wrk's header flag and stays; verified against the option
      table in `cli.rs` and the glibc prefix matcher in
      `getopt.rs`, no existing short or long name collides with
      `http2`
- [ ] ALPN in `TlsSetup` behind `--http2`: offer `h2` and
      `http/1.1` only when the flag is set, pick the connection
      flavor from the negotiated protocol; without the flag the
      TLS config and the wire behavior are unchanged from today
- [ ] `--http2` without h2: fail with a clear error naming what
      the server negotiated, and refuse `--http2` on a plain
      http URL, h2c prior knowledge is out of scope for the
      first cut (decided, see PLAN-http2.md)
- [ ] the h2 framing layer beside `ResponseFramer`: a connection
      state machine over the same `Event` surface with streams in
      place of the pipeline counter
- [ ] `-c` and `-t` semantics decided and documented: `-c` stays
      TCP connections, a new `--streams` flag sets the target
      in-flight streams per connection within the server's
      SETTINGS_MAX_CONCURRENT_STREAMS
- [ ] request mapping: engine request bytes parsed with the
      existing httparse machinery into method, path, and headers,
      connection-specific headers stripped, `Host` to
      `:authority`, the engine contract unchanged
- [ ] stats per stream: one latency sample per response, the rate
      tick unchanged, the timeout accounting restated
- [ ] the capability model note: `Capabilities` stays script
      level, protocol choice is runner level, the pipeline
      probe's meaning under h2 decided and written down
- [ ] docs: README and the engines doc state that h2 is opt-in
      and why, comparability with wrk history, wrk is 1.1 only
- [ ] e2e: a local h2-capable server harness in `tools/`, the
      full existing suite green on the 1.1 path
- [ ] acceptance: an h2 target with `--http2` negotiates h2 and
      completes a run with per-stream stats through every output
      mode, and a run without `--http2` behaves exactly as before,
      byte for byte in legacy

  Notes: effort L, the largest phase in the file. Open questions,
  the framing dependency and pipelined scripts under h2, in
  PLAN-http2.md.

## Not planned

Distributed and cloud load generation. Orchestrating multiple
load generator nodes, merging their results, and running them as
a managed service would put wrkrs in direct competition with k6,
Locust, and Gatling, tools built around that model, while
diluting the focus that makes wrkrs worth reaching for: a
single-node, drop-in wrk replacement with honest measurement. The
coordination layer would dwarf the measurement core in both code
and maintenance. The versioned report of V9 and the npm API of V8
leave the door open for a third party to orchestrate wrkrs nodes
without this project carrying the weight.

