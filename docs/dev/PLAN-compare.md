# wrkrs compare plan

Phase V12, the detail behind the PLAN.md phase. Regression
comparison and thresholds for CI: turn two report files into a
pass/fail verdict with an exit code.

Sequencing (decided): V12 lands on either side of the V5 flip and
never gates it. The subcommand is invoked explicitly, so it is
inherently opt-in and touches nothing until asked. The compare
document carries its own `schema`, and everything the phase adds
to existing reports is additive-only under the V9 policy.

## The problem

A single wrk run is noisy. Teams that want "did this change
regress p99" from wrkrs today have to script repeated runs, pick a
statistic, and roll their own diff of two JSON documents. The
result is home-grown in every repo. wrkrs already emits the
machine readable object; the missing piece is repetition,
aggregation, and a verdict.

## Current state

- `main.rs` dispatches only `Outcome::Run | ListEngines | Usage`
  from `cli::parse`; there is no subcommand surface.
- `-O` writes exactly one report per process invocation.
- `json.rs` emits one pretty-printed object per run; from V9 it
  carries `schema`.
- No aggregation, statistics, or exit-code convention exists
  beyond the wrk-compatible ones (usage exits 1).

## Design

### Command shape

- `wrkrs compare [options] <baseline.json> <new.json>`:
  recognized only as the exact first argument (`argv[1] ==
  "compare"`), never a prefix, abbreviation, or later positional,
  decided. Dispatch happens in `main.rs` before `cli::parse`. A
  benchmark URL can never be the literal `compare`
  (`parse_url` requires a scheme), so plain `wrkrs <options>
  <url>` stays fully wrk-compatible, and a guard test must cover
  that a URL or any wrk-style invocation never dispatches as a
  subcommand.
- Alternatives considered and rejected: a `--compare` flag tangles
  compare options with run options; a second binary splits the
  release artifacts. The subcommand is the first of its kind, so
  the dispatch rule (exact first argument before getopt) gets
  written down in AGENTS.md when it lands.

### Repeated runs

- `--runs <N>` on the run side (default 1): the runner executes
  N times in one process, each run reports independently.
- `--warmup <W>` (default 1): W discarded runs before the counted
  ones, JIT, cache, and connection warmup for the target.
- Output: `--runs` requires `-O`; the file receives N
  newline-delimited JSON objects (NDJSON). `-o json` on stdout
  keeps its one-object contract, repetition goes through the file.

### Aggregation

- Inputs: one object or NDJSON per side, validated against the
  V9 `schema` field; a document with a newer schema than the
  binary refuses with a clear message rather than misreading.
- Per field, fixed and documented:
  - `requests_per_s`: median across runs;
  - `latency.p50/p75/p90/p99`: median across runs of the run
    percentile;
  - error rate: pooled, total errors over total requests, not a
    median of ratios;
  - `bytes_per_s`: median.
- Confidence: a 95% bootstrap interval over runs with a fixed
  seed, deterministic in CI; with fewer than 5 runs the interval
  is reported as unbounded on that side and threshold behavior
  falls back to the point statistic (documented).

### Thresholds and exit codes

- `--threshold '<field><op><value>'`, repeatable, fields `p50`,
  `p75`, `p90`, `p99` (accepts `ms`, `us`, `s` suffixes, plain
  numbers are milliseconds), `requests_per_s` (with `>`/`<`),
  `error_rate` (percent, e.g. `error_rate<0.1%`). Values use the
  same unit scanner family as the CLI.
- A TOML file alternative `--thresholds <file>` for repos that
  keep the gates in version control.
- Exit codes: 0 all thresholds pass, 1 at least one threshold
  failed, 2 usage or input error (unreadable file, unknown
  schema, malformed threshold). CI plugs in directly.

### Compare output

- Terminal: a human table, one row per field, median, interval,
  delta percent against baseline, verdict marks.
- `-o json`: a compare document with its own `schema` (V9
  policy), `{ schema, baseline, new, fields: { ... },
  thresholds: [...], passed }`, the machine contract the V8 npm
  API wraps.

## wrk compatibility

Nothing existing moves: no flag changes, no output format
changes, the run path only gains `--runs/--warmup` opt-ins. The
compare document is a new artifact with its own schema field, it
never masquerades as a run report.

## Tasks

1. Subcommand dispatch in `main.rs` plus the compare skeleton and
   usage text, with the guard test: a URL or any wrk-style
   invocation never dispatches as a subcommand, exact first
   argument only.
2. Report loading: single and NDJSON, schema validation, error
   messages.
3. `--runs`/`--warmup` in the run path, NDJSON writing through
   `-O`.
4. Aggregation module: per-field statistics, seeded bootstrap,
   unit tests over synthetic run sets.
5. Threshold parsing and evaluation, the exit code contract,
   unit tests including malformed input.
6. Terminal table and the JSON compare document with golden
   tests.
7. `tools/compare_e2e.py`: synthetic fixtures with known
   distributions produce deterministic verdicts; a live leg gates
   a deliberately slowed local server and passes a healthy one.
8. Docs: README section, a CI recipe example.

## Acceptance

Synthetic fixtures with hand-computed medians produce the exact
documented verdicts and exit codes; the live e2e fails (exit 1)
against a server with an injected delay and passes (exit 0) the
healthy one; a plain run invocation behaves exactly as before.

## Risks and open questions

- Effort **M**.
- CI machines are shared and noisy; the docs must recommend
  dedicated or self-hosted runners and generous thresholds, and
  the bootstrap interval makes the noise visible rather than
  hiding it.
- Statistics review needed: median-of-run-percentiles is robust
  but not identical to a pooled percentile over all runs; the
  choice must be documented as a definition, not implied.
- Open: default `--warmup` (proposed 1) and whether `--runs`
  without `-O` is an error (proposed yes).
- Open: thresholds binding on the median (proposed) with an
  opt-in `--strict` binding on the interval bound.
- Open: should compare also accept the V10 uncorrected block and
  V11 diagnostics as threshold fields (`diag_warnings==0` is a
  natural CI gate)? Proposed yes once those phases land.
