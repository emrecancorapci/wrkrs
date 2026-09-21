# wrkrs 2.0 Plan

The 2.0 line, ordered by value. `TODO.md` holds the candidate
descriptions, this file holds the execution plan.

Ground rules: small commits, every commit builds and passes
`just lint` and `just test` standalone, both engine legs stay green,
incomplete commits are repaired with fixup and autosquash. Nothing
is pushed without review.

## Phase V1 — the config engine

Scripts without scripts: a data file shapes the load.

- [ ] `crates/wrkrs/src/engines/config/`: parse a TOML or JSON file
      into a request spec through one serde struct
- [ ] schema v1: `[request]` with `method`, `path`, `body`, and a
      headers table, plus `[stop]` with `requests` and `bytes`
      (absent sections mean the defaults)
- [ ] dispatch on the `.toml` and `.json` extensions through the
      existing engine registry, `-e config` overrides by name
- [ ] the engine reports static capabilities for literal requests,
      dynamic once templating lands in V2
- [ ] `-E` lists the config engine alongside the scripting engines
- [ ] acceptance: a `bench.toml` describing a POST runs and reports
      exactly like the equivalent `post.lua`; parse unit tests; the
      script e2e harness grows config file cases

## Phase V2 — templating and stop conditions

- [ ] light templating in `path`, `body`, and header values:
      `{{n}}` is a per thread counter starting at one, `{{rand}}` is
      a random integer, substitution happens per burst so a
      templated file reports dynamic capabilities
- [ ] stop conditions: `stop.requests` stops each thread after N
      completed requests, the stop.lua behavior as a field
- [ ] acceptance: a templated config file behaves like `counter.lua`
      on the wire, a stop config lands on exactly N like `stop.lua`

## Phase V3 — capture rules

- [ ] `[capture]`: copy a response header from one path onto the
      headers of later requests, the auth.lua flow without a script
- [ ] a capture rule turns response buffering on, the wants response
      capability, so the file format stays honest about its cost
- [ ] acceptance: the auth flow as a config file passes the same
      e2e assertions as `auth.lua`

## Phase V4 — the html reporter

- [ ] `-o html`: render the JSON shape into one self contained page,
      inline styles and scripts, no external assets and no network
- [ ] the latency distribution charted, the totals and error blocks
      in place, numbers identical to the JSON mode by construction
- [ ] acceptance: golden test on a synthetic report, valid output
      asserted with a parse

## Phase V5 — modern becomes the default

- [ ] flip the default output mode, `-o legacy` unchanged
- [ ] README, AGENTS.md, and the usage text updates
- [ ] the 2.0 version bump rides with this, the semver promise for
      the default flip

## Phase V6 — the python engine

- [ ] feature `engine-python` via pyo3, dispatch on `.py`, the wrk
      surface exposed as a module, `Value` mapped to native Python
      objects
- [ ] conformance suite passes for the new engine
- [ ] document the GIL serialization for dynamic scripts and the
      linking story, vendored or system Python
- [ ] acceptance: `scripts/*.py` equivalents of the example scripts
      pass the e2e assertions

## Phase V7 — live progress

- [ ] a countdown and a rolling request rate refreshed on the 100 ms
      tick while a run is in flight, only when stdout is a TTY
- [ ] pipes and file captures stay byte clean, the isatty check
      decides
- [ ] acceptance: captured output is identical with and without a
      TTY
