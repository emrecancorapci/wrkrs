# wrkrs diagnostics plan

Phase V11, the detail behind the PLAN.md phase. Load-generator
self-diagnostics: warn when the client, not the server, is the
bottleneck.

Sequencing (decided): V11 lands after the V5 flip and never gates
it, shipping as an opt-in feature behind the `--diagnostics`
flag. Everything it adds to the report is additive-only under the
V9 schema policy; always-on reporting is a later decision once
the false-positive record is proven.

## The problem

A saturated load generator measures itself. CPU-bound threads, a
lagging event loop, imbalanced threads, or exhausted ephemeral
ports and file descriptors all skew results silently, and the user
blames the server. The only hints today are the connect-error
counter (every failure in `connect_slot` folds into
`errors.connect`, including the reconnect storm) and the
benchmarking-tips prose in README. Nothing distinguishes "the
target refused me" from "I ran out of ports".

## Current state

- `eventloop.rs`: the 100 ms rate tick (`RECORD_INTERVAL_MS`,
  `record_rate`) runs on a fixed schedule (`rate_deadline +=`
  100 ms per tick), so tick lateness is already measurable
  without new scheduling. Poll iterations and event-batch
  processing are not timed today.
- `runner.rs`: per-thread counters are merged into totals at the
  join and discarded; imbalance needs them kept per thread until
  after the merge.
- `connect_slot`: any `TcpStream::connect` failure counts
  `errors.connect` and leaves the slot empty; the errno is not
  inspected, so `EADDRNOTAVAIL` (ephemeral ports) and
  `EMFILE`/`ENFILE` (descriptors) are indistinguishable from
  `ECONNREFUSED`.

## Design

### Signals

| Code | Signal | Source |
| --- | --- | --- |
| `eventloop_lag` | tick lateness or poll-to-poll delay sustained above threshold | tick schedule in `eventloop.rs` |
| `cpu_saturation` | busy fraction, time processing events over wall time, sustained above threshold | `Instant` deltas around poll iterations |
| `thread_imbalance` | spread of per-thread `complete` counters at the join | `runner.rs` |
| `ephemeral_ports` | `EADDRNOTAVAIL` connect failures recurring after warmup | errno classification in `connect_slot` |
| `fd_exhaustion` | `EMFILE`/`ENFILE` connect or register failures recurring | errno classification in `connect_slot` |

errno classification reads `Error::raw_os_error()`; the mapping is
written per platform and degrades to no classification where a
code is unknown, so no platform leg gets a false code.

### Avoiding false positives

These rules are part of the contract, not tuning knobs:

- warmup window: the first 2 s of ticks are ignored, the connect
  burst at start is expected to be loud;
- sustained condition: a warning fires only after 5 consecutive
  offending ticks (>= 500 ms), single spikes never warn;
- minimum run: runs shorter than 5 s never warn;
- stop conditions: a `[stop] requests/bytes` file ends threads at
  different counts, imbalance is expected, the check is suppressed
  when a stop condition ran;
- refused connects: `ECONNREFUSED` is a dead target, not a client
  problem, it never feeds the port or fd codes;
- thresholds are constants, documented beside the codes, changed
  only with a documented reason.

### Output format

All reporting rides the opt-in `--diagnostics` flag; the
instrumentation collects always (it is cheap), the warnings and
the report block appear only when asked.

- Text: one line per warning on **stderr**, never stdout, stable
  prefix `wrkrs: warning: <code>: <detail with numbers and a
  remedy>`, for example
  `wrkrs: warning: cpu_saturation: thread 1 busy 96% for 1.4s, raise -t or lower -c`.
- JSON (`json.rs`, additive under the V9 policy): a `diagnostics`
  object `{ "warnings": [{ "code", "detail", "observed",
  "threshold" }] }`, emitted with the flag on, empty when
  healthy.
- HTML (`html.rs`): a warnings block next to the errors block,
  rendered only when non-empty.
- Modern: a Warnings section after the errors line.
- Legacy with `--diagnostics`: warnings on stderr only. Legacy
  stdout is byte-identical whether warnings fire or not, in
  every mode.

## Tasks

1. Tick instrumentation in `eventloop.rs`: lateness and busy
   fraction, collected cheaply, compiled always.
2. Errno classification in `connect_slot`: distinct internal
   counters, still summed into `errors.connect` for compatibility.
3. Per-thread counters preserved through the join in `runner.rs`.
4. The thresholds module: codes, conditions, warmup and
   suppression rules, unit tests over synthetic tick sequences.
5. Output wiring: the `--diagnostics` flag, stderr lines, the
   JSON `diagnostics` object, the HTML block, the modern section.
6. `tools/diagnostics_e2e.py`: an undersized client (-t1 -c50
   against a fast local server) trips cpu/lag; a huge `-c` against
   loopback trips `ephemeral_ports`; a healthy run stays silent.
7. Docs: README section, AGENTS.md notes, the codes table.

## Acceptance

The undersized run warns exactly once per sustained condition in
the documented format; the healthy run produces an empty
`diagnostics` block and no stderr output; `-o legacy` and `-o
modern` stdout are byte-identical with diagnostics collected on
and suppressed.

## Risks and open questions

- Effort **M**.
- Sequencing: after the V5 flip, opt-in behind `--diagnostics`,
  additive-only under the V9 policy, never gates 2.0. The
  warnings-on-by-default question is settled as opt-in for now
  and revisited once the false-positive record is proven.
- Hot-path cost of timing every poll iteration must be measured
  (`perf_sanity.py` is the harness for that check).
- errno portability across the six release legs; unknown codes
  degrade to silence, never to a wrong warning.
- Credibility is the product: one false positive in CI and users
  turn the feature off. The suppression rules above are the
  defense, thresholds stay conservative.
- Open: does the JSON block also carry quiet observability (tick
  lag percentiles) for the V12 compare to consume, or warnings
  only? Proposed warnings only until compare asks.
