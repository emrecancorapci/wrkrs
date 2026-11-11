# wrkrs Lua compatibility plan

Phase V13, the detail behind the PLAN.md phase. Prove wrk script
compatibility with real-world scripts and publish the answer as a
tested table, extending the existing suites rather than building
a parallel one.

## The problem

"Will my existing wrk script run on wrkrs?" deserves a per-API
answer with tests behind it, not folklore. Today the honest answer
is "the nine bundled examples pass, the conformance suite passes,
the Lua scripting doc says the API is the same", which is good but invisible
to a user evaluating the tool, and untested against the scripts
people actually carry around: cookie-session flows, rotating
request arrays, think-time pacing, thread work partitioning.

## Current state

- `crates/wrkrs-engine-tests/src/suite.rs` drives every engine
  through the contract: default request, post, pipeline, delay,
  response capture, done capture, setup/init transfer,
  capabilities honesty, and error propagation (load failure falls
  back to the default request, runtime failure in `request()`
  surfaces). Both Lua builds and QuickJS run it.
- `tools/script_e2e.py` runs the bundled scripts against a local
  Python server, asserts observable behavior, and compares
  against a C `wrk` binary when `./wrk` is present, the harness
  convention the new cases follow.
- `scripts/` holds nine Lua examples (addr, auth, counter, delay,
  pipeline, post, report, setup, stop) with JavaScript twins and
  config-file counterparts.
- `docs/user/scripting/lua.md` is the normative Lua API
  document;
  `crates/wrkrs/src/engines/lua/wrk.lua` is the verbatim wrk
  environment; the AGENTS.md quirks list is the accurate (but
  contributor-facing) known-differences record.

## Design

### Curated real-world scripts

- Patterns to cover, each as an e2e case: header arrays and
  rotating request lists (a table of prebuilt requests walked in
  `request()`), cookie or token session flows (capture on first
  response, replay on later requests, auth.lua's shape driven
  from a script), think-time pacing (`delay()` with varying
  returns), work partitioning through `setup(thread)` and
  `:get`/`:set`, multi-path fan-out via `thread.addr` (addr.lua's
  shape), and custom `done()` reporting over the stats objects
  (report.lua's shape).
- Vendoring policy: scripts originating in the C wrk project are
  Apache-2.0 like `wrk.lua` itself and covered by the existing
  NOTICE attribution. Community scripts from wikis, blogs, and
  gists need a per-script license check before vendoring; when in
  doubt, write an in-repo port that exercises the same API
  surface and name the pattern, not the author.
- Sources are collected in the phase's first PR as a list with
  provenance lines; nothing lands without a license note.

### The supported-surface table

Published user-facing (beside the Lua scripting doc, exact home
decided in the phase), one row per API with status and notes:

| Surface | Items |
| --- | --- |
| wrk table | `scheme`, `host`, `port`, `method`, `path`, `headers`, `body`, `thread`, `addrs` |
| wrk functions | `format`, `lookup`, `connect`, `resolve` (the wrk.lua layer) |
| callbacks | `setup`, `init`, `delay`, `request`, `response`, `done` |
| thread object | `.addr`, `:get`, `:set`, `:stop` |
| stats objects | `.min`, `.max`, `.mean`, `.stdev`, `:percentile(p)`, `obj(i)`, `#obj` |

All rows are supported today; the notes column carries the
behavioral caveats (nul-separated header pairs, `args[0]` is the
URL, `wrk.format` mutating the headers table, `Content-Length`
only with a body).

### Known differences document

User-facing, seeded from AGENTS.md and docs/user/engines.md,
which stay
normative for contributors:

- f64 aggregation where C wrk uses `long double` (rare last-digit
  differences on x86-64; ARM C wrk matches).
- rustls declines TLS 1.0/1.1 and weak ciphers that C wrk's
  OpenSSL method accepts; verification is off in both.
- Script load failure warns and continues with the default
  request (same as wrk); hot-path errors are fatal where wrk
  aborts through unprotected calls, with the message printed.
- A stop can strand one or two in-flight requests (the C binary
  shows the same gap).
- QuickJS-engine differences (undeclared globals throw, lossy
  UTF-8 bodies, insertion-order headers, heap caps) stay in
  docs/user/engines.md and are linked, not duplicated.

### Extend, do not duplicate

- Engine-agnostic API-edge cases go into `wrkrs-engine-tests` so
  the Lua 5.4 and QuickJS legs run them too.
- Behavioral cases with wire assertions go into
  `tools/script_e2e.py`, its wrk-comparison leg active when the C
  binary is present.
- No new suite, no new harness.

## Tasks

1. The curation PR: candidate script list with provenance and
   license notes, decisions recorded.
2. Port or vendor the scripts into `scripts/` with e2e cases.
3. Engine-agnostic API-edge cases in the conformance suite.
4. The supported-surface table published beside the Lua
   scripting doc.
5. The known-differences document, linked from README and the
   Lua scripting doc.
6. e2e comparison assertions for every curated script.

## Acceptance

Every curated script passes e2e on wrkrs; where the harness has
the C wrk binary, the observable behavior (requests on the wire,
script stdout, report numbers within formatting tolerance)
matches; every "supported" row of the table has a test behind it
in the suite or the e2e harness.

## Risks and open questions

- Effort **M**.
- Vendored scripts rot: community scripts encode assumptions
  about servers; each carries a maintenance note and can be
  dropped without ceremony.
- wrk binary availability in CI: the comparison legs skip when
  absent (existing convention), the wrkrs-side assertions are
  unconditional so the suite is always meaningful.
- Open: table home, a COMPATIBILITY section in the Lua
  scripting doc vs a spot in engines.md.
- Open: whether the known-differences document also covers the
  config engine (proposed no, it is not a wrk surface).
