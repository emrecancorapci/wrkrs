# wrkrs

A modern HTTP benchmarking tool: a Rust port of
[wrk](https://github.com/wg/wrk) with pluggable scripting engines.

wrkrs generates significant load from a single multi-core CPU by
combining a multithreaded design with scalable event notification
(epoll, kqueue). The default output and command line are byte
compatible with wrk, so it works as a drop-in replacement.

    wrkrs -t12 -c400 -d30s http://127.0.0.1:8080/index.html

An optional script performs request generation, response processing,
and custom reporting. Lua (LuaJIT, or Lua 5.4 as a build feature) and
JavaScript (QuickJS) ship built in, selected by the script extension
or the `-e` flag.

Beyond wrk it adds `-o/--output legacy|modern|json` report formats
(legacy remains the default) and `-O/--output-file` to write the
report to a file.

The full documentation, the scripting guide, and the engine contract
live in [the repository](https://github.com/emrecancorapci/wrkrs).
