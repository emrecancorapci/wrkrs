# wrkrs packaging plan

Ship wrkrs as an npm package that bundles the release binary and
wraps it in a JavaScript API: `run(options) -> report`. The wrapper
is pure JS with zero runtime dependencies, no FFI, no postinstall
scripts. The only stable contract is the CLI plus `-o json`, which
is already tested, and the binary comes from the existing release
pipeline unchanged.

Precedent: esbuild, biome, and turbo on npm (platform packages as
optionalDependencies), ruff and uv on PyPI (binary in wheel). Users
have accepted binary-in-package at scale.

## Decisions

| Decision | Choice | Rationale |
| --- | --- | --- |
| Main package name | `wrkrs` if claimable, else `wrkrs-js` | `wrkrs` covers both the API and the `bin` shim (`npm i -g wrkrs`); decide in P1, names are first come first served |
| Platform packages | `@wrkrs/<platform>` org if claimable, else `wrkrs-<platform>` | scoped names keep the registry clean, unscoped is the fallback |
| Module format | ESM only, plain JS + JSDoc types, no build step | zero build machinery; a CJS twin is added only if demand appears |
| Binary delivery | inside the platform packages, installed by `optionalDependencies` | no downloads at install time, works offline, avoids the supply-chain and corporate-proxy problems that moved esbuild off postinstall |
| One name, per-platform tarballs | platform packages behind one main package | npm cannot filter `files` by platform, so a single tarball ships all six binaries to everyone; biome ships ten `@biomejs/cli-*` optionals behind `@biomejs/biome`, esbuild and rollup the same shape — the consumer still installs one name |
| Version lock | package version always equals the workspace version, cut from the same tag | one tag cuts binary, tarballs, and npm packages; asserted in CI |
| Publish gate | a release workflow triggered when a GitHub release is published | the existing pipeline drafts a release for review; npm publish fires only after a human publishes it, matching the repo rule that nothing ships without review |
| Windows | out of scope for this line | no Windows build leg exists; `os`/`cpu` fields make installs fail with a known message; see the research section |
| PyPI, NuGet | follow-ups once npm proves the flow | same artifacts, same pipeline shape |

## The package shape

```
packages/node/                  main package: API, types, bin shim
@wrkrs/linux-x64-gnu/           binary from the linux-gnu release leg
@wrkrs/linux-arm64-gnu/
@wrkrs/linux-x64-musl/          static, Alpine and distroless containers
@wrkrs/linux-arm64-musl/
@wrkrs/darwin-x64/
@wrkrs/darwin-arm64/
```

Platform packages carry `os`, `cpu`, and `libc` fields (`glibc` vs
`musl`, which disambiguates the linux twins), the binary at
`bin/wrkrs`, plus LICENSE and NOTICE. The Apache attribution must
ship in every package.

Consumers see one name: `npm install wrkrs`. The platform packages
are `optionalDependencies`, invisible implementation details that
npm resolves from `os`, `cpu`, and `libc` — the install experience
biome and esbuild ship today, not a package users enumerate
themselves.

A literal single tarball carrying all six binaries is the honest
alternative: the linux-gnu x64 binary measures 4.2MB stripped,
2.1MB gzipped, so one-size-fits-all costs roughly 13MB downloaded
and 30MB unpacked per install, most of it dead weight on any given
machine, and it grows linearly with every platform added. The shape
decision is revisited once at P3 with the real binaries measured,
not defended on estimates.

### The API

```js
import { run } from 'wrkrs'

const report = await run({
  url: 'http://127.0.0.1:8080/',
  threads: 2,
  connections: 50,
  duration: '10s',              // '10s' | '2m' | '1h' | seconds as number
  headers: { 'User-Agent': 'wrkrs-js' },
  request: {                    // serialized to a temp .json, passed as -s
    method: 'POST',
    path: '/api',
    body: 'foo=bar',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
  },
  stop: { requests: 10_000 },   // the [stop] fields, same file
  script: undefined,            // or a script file path, mutually exclusive with request/stop
  timeout: 2000,                // socket --timeout in ms
  signal: controller.signal,    // AbortSignal -> SIGINT
  timeoutMs: 120_000,           // hard child timeout -> kill -> error
  bin: undefined,               // binary override, the dev and test hook
  extraArgs: [],                // appended verbatim, e.g. ['-e', 'config']
})
```

Behaviors, fixed by tests:

- always `-o json`; the report is the parsed stdout object
  (`duration_us`, `requests`, `bytes`, `requests_per_s`, `bytes_per_s`,
  `errors{connect,read,write,timeout,status}`, `latency` and `rate`
  blocks with percentiles on latency, `version`), latency in
  microseconds
- `request`/`stop` serialize to the config engine JSON shape
  (`scripts/bench.json` is the schema) into a temp `.json` file,
  removed in `finally`
- abort sends SIGINT; the runner stops the run gracefully and the
  reporter still runs, so an aborted run resolves with its partial
  report when stdout parses
- error taxonomy: binary or platform package missing (with an
  install hint), non-zero exit with stderr captured, hard timeout
  after kill
- binary resolution order: `bin` option or `WRKRS_BIN` env, then
  `@wrkrs/<platform>` resolved from `process.platform`,
  `process.arch`, and glibc detection
  (`process.report.getReport().header.glibcVersion`) for musl, then
  a clear error naming the expected package
- a `bin/wrkrs` shim execs the resolved binary with inherited stdio,
  so `npx wrkrs` and `npm i -g wrkrs` give the plain CLI

## Ground rules amendment

`just lint` and `just test` stay Rust-only, no node requirement
appears for Rust work. The package gets its own recipes, `just
package-test` (unit plus integration against a local `node:http`
server, binary from `WRKRS_BIN` defaulting to
`target/release/wrkrs`) and `just package-pack` (npm pack hygiene),
wired into CI as a `node` job that builds the binary first. Commits
that touch `packages/` must pass the package recipes; commits that
do not are unaffected. Everything else in the ground rules holds:
small commits, every commit green standalone.

## Phases

### P1 — the contract and scaffold

- [ ] `packages/node/` skeleton: `"type": "module"`, `exports`,
      `engines.node >= 18`, zero dependencies, JSDoc types
- [ ] minimal `run()`: url, threads, connections, duration, spawn,
      parse `-o json`, `bin`/`WRKRS_BIN` override
- [ ] unit tests for the argument map and the report parse (golden
      fixtures), integration test against a local `node:http` server
- [ ] `just package-test`; a `node` job in `build.yml` (ubuntu and
      macos, node 20 and 22) building the binary then running it
- [ ] resolve the npm name decisions, claim the name and the org
- acceptance: the job is green on both engine legs' runners with a
  freshly built binary

### P2 — the full API

> Note (2.0 planning): P2 also carries the typed test API from
> the V8 phase in docs/dev/PLAN.md, corrected, uncorrected, and
> send-time latency (`latency_uncorrected`, `latency_send_time`),
> diagnostics, and compare/threshold results in assertion-friendly
> types. It depends on phases V9-V12 landing first.

- [ ] `headers`, `request`, `stop` through the temp config file;
      `script`, `timeout`, `extraArgs` passthrough
- [ ] `signal` abort to SIGINT, `timeoutMs` hard kill, the error
      taxonomy, `.d.ts` emitted from JSDoc
- [ ] the `bin/wrkrs` shim, executable bit and `files` allowlist
      checked by an `npm pack` + install-from-tarball test
- acceptance: an aborted run resolves with a partial report; a clean
  install from the packed tarball passes the integration test;
  `node packages/node/bin/wrkrs.js -E` lists engines

### P3 — platform packages and resolution

- [ ] a generator script producing the six platform packages from a
      release tarball set: package.json with `os`, `cpu`, `libc`,
      the binary, LICENSE, NOTICE
- [ ] the shape gate: pack the real binaries once the musl and
      darwin legs exist, record the combined gzipped size, and keep
      platform packages unless the single-tarball total is trivial
      (under ~8MB gzipped) and the platform set is frozen
- [ ] runtime resolution with musl detection and the not-found error
      carrying the install hint (the Windows message names the
      missing platform support explicitly)
- acceptance: installing the packed tarballs together in a temp
  project resolves and runs on the runner; forcing an unmatched
  platform fails with the hint, not a stack trace

### P4 — the publish pipeline

- [ ] `.github/workflows/publish-npm.yml` on release `published`,
      plus a `workflow_dispatch` rehearsal mode that stops before
      `npm publish`
- [ ] assert tag equals the workspace version; download the six
      release tarballs and verify SHA256SUMS
- [ ] generate packages from the verified tarballs, smoke test by
      installing all tarballs into a temp project and running the
      integration suite, publish platform packages then the main
      package, `--provenance` with the OIDC token
- acceptance: the rehearsal publishes nothing and fails closed on a
  version mismatch; the first real release ships all seven packages

### P5 — docs

- [ ] `packages/node/README.md`: install, quickstart, options and
      report reference, error handling, abort semantics, CLI usage
      through the bin shim, Windows status
- [ ] root README gains a Packages section pointing there; this
      document flips to status notes
- acceptance: a fresh-machine install from npm passes the README
  quickstart verbatim

## Research and follow-ups

- **PyPI and NuGet**: the same verified tarballs feed wheels (the
  ruff model, binary in wheel, `pipx install wrkrs`) and a NuGet
  `runtimes/{rid}/native/` package. The npm pipeline is the
  template, P4 generalizes.
- **Windows**: a build leg first (the event loop is epoll/kqueue
  shaped, mio has windows support but nothing is wired), then two
  more platform packages. Decide on demand.
- **napi backend**: if process spawn ever matters (it will not for
  multi-second runs), the main package swaps its internals for a
  native addon without changing the public API.

## Failure and rollback notes

- npm versions are immutable: a bad publish is deprecated, fixed as
  a patch release. The release notes template says so.
- A re-pushed tag re-runs the binary pipeline and clobbers release
  assets, but the npm publish for an already-published version fails
  closed; recovery is always a version bump.
- Binary size lands in the release notes per platform; the six
  platform packages keep the main package small.
