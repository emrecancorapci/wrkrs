# wrkrs 2.0 TODO

Candidate features for the 2.0 line. Short descriptions, decide the
order when the line starts.

## Output

- **html reporter**: render the JSON shape into one self contained
  page with the latency distribution charted. The v1 JSON schema was
  designed as its input.
- **modern becomes the default**: `legacy` stays available behind
  `-o legacy`, the drop-in compatibility promise holds per major.
- **live progress**: a countdown and a rolling request rate on the
  100 ms tick while a run is in flight, only when stdout is a TTY so
  pipes and captures stay clean.

## Declarative request config

Scripting without scripting: describe the load in a data file.

- **config engine**: a TOML or JSON file shapes the request, method,
  path, headers, body, without any script. Dispatched by file
  extension (`-s bench.toml`) and implemented as an engine against
  the wrkrs-engine contract, so stats, reporting, and pipeline
  detection behave exactly like the scripted engines. TOML is the
  human first format, JSON rides along for tooling.
- **light templating**: counters and random numbers in paths and
  headers, `{{n}}` and `{{rand}}`, covering the counter.lua style
  dynamic scripts.
- **stop conditions**: stop after N requests or N bytes, the
  stop.lua behavior as a field.
- **response capture rules**: copy a header from one path onto later
  requests, the auth.lua flow without a script. The point where a
  data file starts becoming a language, keep the rule shape strict.

The config mode complements the engines, it does not replace them:
response callbacks with arbitrary logic, computed requests, and
custom done reporting stay in scripts.

## Engines

- **python engine**: pyo3 hosting CPython behind the same contract,
  feature `engine-python`, dispatching on `.py`. Python is the
  biggest scripting audience for load tools (locust users) and the
  binding cost is one crate: the Value type maps to native Python
  objects and the wrk table becomes a module. Caveats to document:
  the GIL serializes per thread callbacks under dynamic scripts
  (static scripts never call back at runtime, so full rate holds),
  and the build needs a Python to link, vendored or system, the
  LuaJIT style static story is heavier here.

## Research

- **HTTP/2**: httparse is 1.1 only, an h2 engine means a second
  framing layer beside the response framer. Decide after measuring
  demand, the C tool never had it.
