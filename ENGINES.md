# Scripting Engines

wrkrs drives benchmark scripts through pluggable scripting engines. Lua
is the classic surface, JavaScript is available out of the box, and the
`wrkrs-engine` contract lets other languages plug in.

## Selecting an engine

Without flags the engine is chosen by the script file extension:

```
wrkrs -t2 -c10 -d10s -s bench.lua http://127.0.0.1:8080/   # Lua engine
wrkrs -t2 -c10 -d10s -s bench.js  http://127.0.0.1:8080/   # QuickJS
```

`-e, --engine <name>` selects an engine explicitly and overrides the
extension, `-E, --engines` lists the engines compiled into the binary:

```
$ wrkrs -E
  luajit     .lua    LuaJIT 2.1 (vendored)
  quickjs    .js     QuickJS (rquickjs)
```

Failure cases carry fixed messages:

| Situation | Message |
| --- | --- |
| `-e` without `-s` | `option -e requires a script (-s)` |
| Known engine not compiled in | `engine 'lua54' not compiled in, rebuild with --features engine-lua54` |
| Unknown engine name | `engine 'x' not compiled in, compiled engines: ...` |
| No engine handles the extension | `no engine handles the 'txt' extension, compiled engines: ...` |
| Script without an extension | `'bench' has no extension, compiled engines: ...` |

## Compiled-in engines

| Name | Extension | Runtime | Build |
| --- | --- | --- | --- |
| `luajit` | `.lua` | LuaJIT 2.1, vendored | default features |
| `lua54` | `.lua` | Lua 5.4, vendored | `--no-default-features --features engine-lua54,engine-quickjs` |
| `quickjs` | `.js` | QuickJS via rquickjs | default features |

Engines register at compile time through cargo features, so the binary
always knows exactly what it runs and script dispatch never guesses.

`engine-luajit` and `engine-lua54` are mutually exclusive: mlua links one
Lua runtime per binary and the runtimes export clashing symbols. Building
with both fails with a `compile_error!` that says so. Both Lua builds run
the same engine implementation and the same conformance suite, and `.lua`
dispatches to whichever Lua runtime the build carries.
