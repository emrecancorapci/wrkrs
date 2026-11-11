# Lua scripting

wrkrs executes a Lua script during three distinct phases: setup,
running, and done. Each thread has an independent scripting
environment, and the setup and done phases execute in a separate
environment which does not participate in the running phase.

The public Lua API consists of a global table and a number of
global functions:

```lua
wrk = {
  scheme  = "http",
  host    = "localhost",
  port    = nil,
  method  = "GET",
  path    = "/",
  headers = {},
  body    = nil,
  thread  = <userdata>,
}
```

## Functions

### wrk.format(method, path, headers, body)

`wrk.format` returns an HTTP request string containing the passed
parameters merged with values from the `wrk` table.

### wrk.lookup(host, service)

`wrk.lookup` returns a table containing all known addresses for
the host and service pair. This corresponds to the POSIX
`getaddrinfo()` function.

### wrk.connect(addr)

`wrk.connect` returns `true` if the address can be connected to,
otherwise it returns `false`. The address must be one returned
from `wrk.lookup()`.

## Callbacks

The following globals are optional, and if defined must be
functions:

| Global | Called |
| --- | --- |
| `setup` | during thread setup |
| `init` | when the thread is starting |
| `delay` | to get the request delay |
| `request` | to generate the HTTP request |
| `response` | with HTTP response data |
| `done` | with results of the run |

## Setup

```lua
function setup(thread)
```

The setup phase begins after the target IP address has been
resolved and all threads have been initialized but not yet
started.

`setup()` is called once for each thread and receives a userdata
object representing the thread:

```lua
thread.addr             -- get or set the thread's server address
thread:get(name)        -- get the value of a global in the thread's env
thread:set(name, value) -- set the value of a global in the thread's env
thread:stop()           -- stop the thread
```

Only boolean, nil, number, and string values, or tables of the
same, may be transferred via `get()`/`set()`, and `thread:stop()`
can only be called while the thread is running.

## Running

```lua
function init(args)
function delay()
function request()
function response(status, headers, body)
```

The running phase begins with a single call to `init()`, followed
by a call to `request()` and `response()` for each request cycle.

The `init()` function receives any extra command line arguments
for the script, which must be separated from wrk arguments with
`--`.

`delay()` returns the number of milliseconds to delay sending the
next request.

`request()` returns a string containing the HTTP request. Building
a new request each time is expensive; when testing a high
performance server, one solution is to pre-generate all requests
in `init()` and do a quick lookup in `request()`.

`response()` is called with the HTTP response status, headers, and
body. Parsing the headers and body is expensive, so if the
`response` global is nil after the call to `init()`, wrkrs ignores
the headers and body.

## Done

```lua
function done(summary, latency, requests)
```

The `done()` function receives a table containing result data, and
two statistics objects representing the per-request latency and
the per-thread request rate. Duration and latency are microsecond
values, and rate is measured in requests per second.

```lua
latency.min              -- minimum value seen
latency.max              -- maximum value seen
latency.mean             -- average value seen
latency.stdev            -- standard deviation
latency:percentile(99.0) -- 99th percentile value
latency(i)               -- raw value and count
```

```lua
summary = {
  duration = N,  -- run duration in microseconds
  requests = N,  -- total completed requests
  bytes    = N,  -- total bytes received
  errors   = {
    connect = N, -- total socket connection errors
    read    = N, -- total socket read errors
    write   = N, -- total socket write errors
    status  = N, -- total HTTP status codes > 399
    timeout = N  -- total request timeouts
  }
}
```

## Other engines

This document describes the Lua API. wrkrs also runs scripts on
other engines, selected by the `-s` file extension or with `-e`.
See [engines.md](../engines.md) for the engine list, the
JavaScript surface, and how to plug in more.
