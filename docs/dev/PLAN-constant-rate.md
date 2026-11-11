# wrkrs constant-rate plan

Phase V10, the detail behind the PLAN.md phase. True open-loop
measurement with send-time accounting: let a user hold a request
rate the way wrk2 does, measuring latency from the intended send
rather than patching it up after the fact the way wrk's
approximate average-interval correction does.

## The problem

wrkrs is closed loop only. A connection sends its next burst when
the previous one drains (`connection.rs` re-arms writable in
`rearm_or_close`), so when the server stalls, the client stops
asking and the stall mostly disappears from the latency
distribution. The only pacing that exists is the script `delay()`
callback, which sleeps between bursts rather than scheduling
sends.

The framing of this phase matters. wrk is not uncorrected: it
applies an approximate, after-the-fact coordinated-omission
correction over the average interval, and the port keeps that
behavior byte for byte — `runner.rs` `execute()` computes
`duration_us / (complete / connections)` and calls
`latency.correct(interval)`, the `stats.rs` byte-parity port of
`stats_correct`, golden-tested against `tools/golden_stats.txt`.
The goal here is not "correction versus none", it is true
open-loop measurement with send-time accounting beside that
approximation, which has two limits:

- it is an average-interval heuristic over samples that were
  already omitted, and only makes sense for the closed loop that
  produced them;
- it runs unconditionally and mutates the one shared histogram in
  place, before any reporter or the script `done()` callback reads
  it, so no output mode can show the uncorrected numbers and no
  mode ever has.

## Design

### CLI

- `-R, --rate <N>`: total requests per second across all threads,
  SI units through `scan_metric` (`-R 10k`), matching wrk2's flag
  spelling. The short `-R` is free in the `cli.rs` option table.
- Absent `-R`, nothing changes: closed loop, the unconditional
  interval correction, byte-identical output.
- Validation: `--rate` must be >= 1 and >= threads (each thread
  schedules at `rate / threads`, the per-thread interval is
  `threads * 1_000_000 / rate` microseconds, rounded residue
  carried forward so the total rate stays honest).
- Decided: `-R` with `-o legacy` is refused with a clear error
  naming a non-legacy mode, and until the V5 flip makes modern
  the default that means a rate run names its output mode
  explicitly. Rationale: legacy is a byte-compatibility promise,
  refusing is easy to loosen later, the reverse would be
  breaking.

### Loop model

- Open loop: sends are scheduled, responses are not waited for.
  Per thread a next-send deadline advances by the per-thread
  interval and rides the existing `Deadline` heap in
  `eventloop.rs`. The heap's `token` field currently indexes
  connection slots; the send timer takes a reserved token (or the
  timer type grows a variant), decided in implementation.
- Connections: `-c` stays the ceiling. A scheduled send grabs an
  idle connection, opens a new one when none is idle and the
  ceiling allows, and queues (recording the queuing lag, which
  feeds the V11 diagnostics) when the ceiling is reached.
- Latency under a rate: the clock starts at the intended send
  time, not the actual write. That is the correction wrk2 applies
  by construction.

### The three numbers, explicitly

A rate run can report three latency numbers, and which is
primary per output mode is part of the contract:

| Number | Meaning | Present in |
| --- | --- | --- |
| wrk-style corrected | histogram after the average-interval `correct()` patch-up | closed loop |
| uncorrected | raw histogram as recorded, before any correction | both loops |
| send-time | open loop, latency measured from the intended send time | `-R` runs |

- json: `latency` is the primary block and is mode-dependent,
  wrk-style corrected in the closed loop, send-time under `-R`,
  which needs no further patch-up; `latency_uncorrected` is
  always the raw histogram, in both loops.
- modern: the primary distribution prints first, same rule as
  json `latency`, the other labeled beneath it.
- html: same primary as json, both series charted.
- legacy: never sees a rate run, the refusal is decided above;
  the closed loop prints the wrk-style corrected histogram
  exactly as today.

Other schema fields under the V9 policy:

- `mode`: `{ "loop": "closed" | "open", "rate": <u64 | null>,
  "correction": "wrk-interval" | "send-time" }`.
- `latency`: the primary block, mode-dependent as above.
- `latency_uncorrected`: the raw measured view, always.
- `modern.rs`: both distributions printed, the primary first.
  `html.rs`: the second series on the chart.

### Mode naming, stated unambiguously

- The default closed loop is **the wrk-compatible mode**: wrk's
  sampling, wrk's approximate interval patch-up, byte-identical
  output.
- `-R` is **the open-loop mode**: constant rate, send-time
  accounting, comparable to wrk2 (Gil Tene's fixed-rate fork),
  which is the reference behavior for it.

## wrk compatibility

- No `-R` changes nothing anywhere; the golden and e2e legs stay
  green untouched.
- `-o legacy` never sees a rate run: `-R` with `-o legacy`, or
  with the legacy default of 1.x before the V5 flip, fails with
  the documented error. The closed loop through legacy is
  byte-identical as always.
- Comparison legs in `tools/`, following the `script_e2e.py`
  convention of comparing when the reference binary is present:
  closed loop against a C `wrk` (already covered by the existing
  legs), rate runs against a `wrk2` binary when one is available,
  asserting achieved rate within a few percent and the send-time
  percentile magnitudes, never exact numbers — the schedulers and
  timers differ between the tools.

## Tasks

1. `runner.rs::execute()`: snapshot the latency histogram before
   `correct()` runs and carry raw and corrected in `RunReport`,
   no output change. Every reporter still reads the corrected
   histogram exactly as today, and the golden stats test in
   `stats.rs` drives `Histogram::correct` directly and is
   untouched, so `-o legacy` stays byte-identical.
   `latency_uncorrected` cannot exist without this step, so it is
   first.
2. `cli.rs`: parse and validate `-R/--rate`, usage line, `Config`
   field, default none, plus the `-o legacy` refusal and its
   error message.
3. `eventloop.rs`: the send scheduler, reserved timer token,
   idle-connection selection, on-demand connects, queue at the
   ceiling.
4. `connection.rs`: mode-aware `started` — intended send time
   under a rate, actual burst start otherwise.
5. `json.rs` + `modern.rs` + `html.rs`: `mode` and
   `latency_uncorrected`, both distributions with the primary
   first, golden updates.
6. `tools/rate_e2e.py`: fixed-rate assertions against the local
   harness, wrk2 comparison leg.
7. Docs: README options, AGENTS.md behavior notes, the mode
   naming above.

## Acceptance

Against the local harness with a deliberately injected 100 ms
stall: `-R 1000` holds ~1000 req/s while the send-time percentiles
show the stall and the uncorrected ones mostly do not; `-R` with
`-o legacy`, and `-R` under the 1.x legacy default, both fail
with the documented error; a run without `-R` produces
byte-identical output to the current binary; with a wrk2 binary
present the achieved rate and send-time p99 agree within the
documented tolerance.

## Risks and open questions

- Effort **L**, the largest pre-2.0 phase.
- Intended-send bookkeeping is per queued request, not per
  connection; memory is bounded by `-c` times pipeline depth but
  the accounting must not slow the hot path.
- Pipelined scripts under a rate: one scheduled send delivers the
  N-request burst, or N sends of one request? wrk2 keeps the
  script's pipeline depth per send; proposed the same, open.
- Timeouts: a stalled response under open loop routinely exceeds
  the histogram limit (`record` returns false, counted as
  timeout, the wrk rule). Keep and document, or grow the limit
  under `-R`. Open.
- Decided (was open): `-R` with `-o legacy` refused initially,
  see the CLI section; the `--latency` legacy percentile question
  dissolves with it, legacy never sees a rate run.
