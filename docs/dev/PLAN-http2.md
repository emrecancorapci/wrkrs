# wrkrs HTTP/2 plan

Phase V14, the detail behind the PLAN.md phase. Speak h2 where
the server negotiates it, as a 2.x phase after the 2.0 line,
with HTTP/3 an explicitly later phase after this one.

## The problem

Modern servers speak HTTP/2, and measuring them over 1.1 measures
the wrong protocol: no multiplexing, one in-flight burst per
connection, head-of-line blocking at the TCP layer that h2
specifically removes. wrk never grew h2; wrkrs carrying it is a
real differentiator, but it touches the connection core, so it is
sequenced after the 2.0 line settles, not inside it.

## Current state

- `crates/wrkrs/src/parser.rs` `ResponseFramer` frames HTTP/1.1
  responses only (Content-Length, chunked, until-close over
  httparse). The TODO.md Research entry already states the shape: "an h2
  engine means a second framing layer beside the response
  framer".
- `crates/wrkrs/src/tls.rs` builds its rustls `ClientConfig`
  with no `alpn_protocols`, so an https target always lands on
  HTTP/1.1 today (verified: no ALPN anywhere under `crates/`).
- `crates/wrkrs/src/connection.rs`: one `Connection` per slot
  with a single request buffer, a `pending` pipeline counter, and
  latency recorded once per drained pipeline; `eventloop.rs`
  gives each thread `connections/threads` TCP slots.
- Engines produce raw 1.1 request bytes (`ScriptEngine::
  request() -> Vec<u8>`); `verify_request` re-parses those bytes
  with httparse to detect pipeline depth, so the structured
  method/path/headers are recoverable from engine output without
  changing the contract.

## Design

### Negotiation and opt-in

- HTTP/1.1 stays the default for every run. wrk is 1.1 only, and
  existing https users' results must stay comparable with their
  own history, so h2 is requested with an explicit `--http2` and
  is never implied by the URL or the server.
- `--http2` is long-only, no short form. `-H` is wrk's header
  flag and stays; verified against the option table in `cli.rs`
  and the glibc exact-or-unambiguous-prefix matcher in
  `getopt.rs` — no existing short or long name collides with
  `http2` (`help` and `header` diverge after `he`, and nothing
  else begins with `ht`).
- With `--http2`, `TlsSetup` offers `h2` then `http/1.1` via
  ALPN; after the handshake `alpn_protocol()` selects the
  connection flavor per slot. Without the flag no ALPN extension
  is sent, the TLS config and the wire behavior are unchanged
  from today.
- Decided: when `--http2` is set and the server does not
  negotiate h2, the run fails with a clear error naming what was
  negotiated. The flag is an explicit request for h2 numbers;
  silent fallback to 1.1 would hand the user 1.1 measurements
  labeled as h2, reintroducing exactly the comparability problem
  the opt-in exists to avoid. A separate explicit fallback flag
  can be added later without breaking anyone; removing a silent
  fallback would be breaking. (Open: whether that fallback flag
  is ever worth carrying.)
- Decided: `--http2` on a plain http URL (h2c prior knowledge) is
  out of scope for the first cut and refused with a clear error.
  There is nothing to negotiate from, and the first cut serves
  the https case.

### Connections and streams

- `-c` keeps its meaning: TCP connections, split across threads
  as today. `-t` unchanged. This is the honest wrk-compatible
  reading, and it is what every wrk user assumes the flag means.
- Concurrency inside a connection moves to streams: the target
  in-flight streams per connection is `min(--streams <N>, server
  SETTINGS_MAX_CONCURRENT_STREAMS)`, with the server's setting
  discovered from the first connection and respected. Default
  for `--streams`: the server's maximum, capped (proposed cap
  256, tuned in the phase). When the cap binds, the V11
  diagnostics get a code for it.
- `-c` no longer equals in-flight requests under h2; the docs
  say so plainly, because it did not under pipelining either.

### The framing layer

- An h2 connection state machine beside `ResponseFramer`,
  implementing the same `Event` surface (`Interest`, `Reconnect`,
  `Delay`, `Fatal`) so the event loop stays generic. Streams
  replace the `pending` pipeline counter; flow control
  (WINDOW_UPDATE) and GOAWAY handling live here. GOAWAY drains
  then reconnects, the natural analog of `Connection: close`.
- Request mapping: engine bytes parsed with the existing
  httparse request machinery into method, path, authority, and
  headers; `Host` becomes `:authority`; connection-specific
  headers (`Connection`, `Keep-Alive`, `Proxy-Connection`,
  `Transfer-Encoding`, `Upgrade`) are stripped; body length
  follows `Content-Length` or the body bytes verbatim. The
  engine contract, `wrk.format`, and every script stay untouched.

### Stats and capabilities

- Latency: one sample per stream response (request sent to final
  frame), where 1.1 records once per drained pipeline. The rate
  histogram and its 100 ms tick are untouched. Timeout accounting
  restated per stream. `bytes` keeps counting decrypted bytes
  read from the transport, frames included, for continuity with
  the TLS path counting application bytes today.
- `Capabilities` (is_static, wants_response, has_delay, has_done)
  stays script-level and unchanged; protocol selection is a
  runner concern and gets a paragraph in docs/user/engines.md
  saying so.
- Pipeline probe under h2: a script whose request bytes contain
  N pipelined requests targets N concurrent streams per
  scheduled send, preserving script intent. Decision recorded in
  this phase, flagged for review.

## wrk compatibility

- Baseline recorded: `tls.rs` sets no ALPN today, so every https
  run wrkrs has ever made is HTTP/1.1, and no `-o` mode or
  report has ever carried h2 numbers. Until V14 ships, the
  project makes no HTTP/2 parity claims and publishes no
  comparisons against h2load or oha.
- Every 1.1 path (plain, TLS without h2, h2 refused) behaves
  exactly as today; the entire existing suite and the wrk
  comparison legs must stay green untouched.
- No existing flag changes meaning. New surface: `--http2` and
  `--streams`, both opt-in, both absent by default; a run without
  `--http2` is identical to today's binary, TLS config, ALPN,
  framing, and all.
- The README and engines doc updates state why h2 is opt-in:
  comparability with wrk history, wrk is 1.1 only.
- `-o legacy` under h2 prints the standard layout over the
  per-stream histogram; byte-format compatibility is about the
  layout, not the protocol underneath.

## Tasks

1. `--http2` in `cli.rs`, long-only, no short form, with the
   collision check recorded against the option table and the
   prefix matcher, and the no-h2 failure and plain-http refusal
   errors.
2. ALPN in `TlsSetup` gated on `--http2`, with protocol selection
   at slot level, the no-flag path verified byte-identical.
3. The h2 framing layer: connection state machine, streams, flow
   control, GOAWAY, over the existing `Event` surface.
4. Request mapping from engine bytes (httparse extraction,
   header rules) with unit tests over the mapping table.
5. `--streams` flag, server SETTINGS discovery, the concurrency
   scheduler and its cap.
6. Stats per stream, timeout restatement, bytes accounting note.
7. `tools/h2_e2e.py`: a local h2-capable server harness (the
   Python harness today is 1.1 only; the server side needs an h2
   speaking twin or a small Rust test server, decided in phase).
8. Docs: README options and the engines.md capability paragraph
   stating that h2 is opt-in and why, comparability with wrk
   history; AGENTS.md notes; the `-c` semantics statement.

## Acceptance

An h2 target with `--http2` negotiates h2 and completes a run
with per-stream stats through every output mode, `--streams`
bounds observed concurrency to the server's setting, a run
without `--http2` behaves exactly as before byte for byte in
legacy, and `--http2` against a 1.1-only server and against a
plain http URL both fail with the documented errors.

## Risks and open questions

- Effort **L**, the largest phase in the plan.
- Framing dependency: the `h2` crate vs `hyper` is the first
  decision; `h2` alone keeps the event loop ours, `hyper` brings
  a runtime we do not use. Open.
- The 2.0 line must not wait on this: anything that destabilizes
  the 1.1 core lands behind the phase, not inside it.
- HTTP/3 is an explicitly later phase, after V14 proves the
  second framing layer: QUIC over UDP needs a datagram transport
  beside the stream-shaped `Socket` seam and the rustls QUIC
  integration, a transport project on its own. It is not
  scheduled here beyond keeping the seams open.
- Open: whether an explicit fallback flag for `--http2` without
  h2 is ever wanted (fail-closed is decided for the first cut).
- Open: default `--streams` cap. Open: whether pipelined scripts
  under h2 are honored or refused. Open: the h2 test server
  shape.
