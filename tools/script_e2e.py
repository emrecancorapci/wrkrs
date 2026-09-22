#!/usr/bin/env python3
"""Run the bundled scripts end to end against a local server.

Every script from scripts/ drives a different runner path: thread
addresses and the resolver API, dynamic requests, the response
callback, the delay timer, pipelining, the stop flag, and the done
phase. The harness asserts the observable behavior of wrkrs and,
where the report parses, of the C binary for comparison.

Usage: tools/script_e2e.py [wrkrs-binary]
"""

import os
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time

WRKRS = sys.argv[1] if len(sys.argv) > 1 else "./target/release/wrkrs"
WRK = "./wrk"

RESPONSE = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n"
AUTH_RESPONSE = b"HTTP/1.1 200 OK\r\nX-Token: abc123\r\nContent-Length: 2\r\n"

seen_lock = threading.Lock()
seen = {"paths": [], "counters": [], "tokens": [], "posts": []}


def start_server():
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)
    threading.Thread(target=accept_loop, args=(listener,), daemon=True).start()
    return listener.getsockname()[1]


def accept_loop(listener):
    while True:
        conn, _ = listener.accept()
        threading.Thread(target=handle, args=(conn,), daemon=True).start()


def handle(conn):
    buffer = b""
    try:
        while True:
            data = conn.recv(65536)
            if not data:
                return
            buffer += data
            while b"\r\n\r\n" in buffer:
                head, buffer = buffer.split(b"\r\n\r\n", 1)
                lines = head.split(b"\r\n")
                request = lines[0].decode("latin1")
                method, path = request.split(" ")[0], request.split(" ")[1]
                headers = {}
                for line in lines[1:]:
                    if b":" in line:
                        key, value = line.split(b":", 1)
                        headers[key.decode().strip().lower()] = value.decode().strip()
                body_len = int(headers.get("content-length", "0"))
                body = buffer[:body_len]
                buffer = buffer[body_len:]
                with seen_lock:
                    seen["paths"].append(path)
                    if "x-counter" in headers:
                        seen["counters"].append(int(headers["x-counter"]))
                    if "x-token" in headers:
                        seen["tokens"].append(headers["x-token"])
                    if method == "POST":
                        seen["posts"].append(body.decode("latin1"))
                if path == "/authenticate":
                    conn.sendall(AUTH_RESPONSE + b"\r\nok")
                else:
                    conn.sendall(RESPONSE + b"\r\nok")
    except OSError:
        pass


def run(binary, port, script, extra=()):
    start = time.monotonic()
    result = subprocess.run(
        [binary, "-t1", "-c1", "-d1s", "-s", script, *extra, f"http://127.0.0.1:{port}/"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    elapsed = time.monotonic() - start
    return result, elapsed


def wrkrs_requests(result):
    """Pulls the completed count from the report footer."""
    match = re.search(r"  (\d+) requests in", result.stdout)
    return int(match.group(1)) if match else -1


def wrk_requests(result):
    match = re.search(r"(\d+) requests in", result.stdout)
    return int(match.group(1)) if match else -1


def check(name, condition, detail=""):
    if condition:
        print(f"  ok: {name}")
    else:
        print(f"  FAIL: {name} {detail}")
        return False
    return True


def main():
    port = start_server()
    failures = 0

    # The setup script wants two threads to show the cross context
    # traffic, so it runs on its own.
    with seen_lock:
        for key in seen:
            seen[key].clear()
    result = subprocess.run(
        [WRKRS, "-t2", "-c2", "-d1s", "-s", "scripts/setup.lua", f"http://127.0.0.1:{port}/"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    ok = check("setup: exit zero", result.returncode == 0, result.stderr[-200:])
    ok &= check("setup: both threads created", result.stdout.count("created") == 2, result.stdout)
    pairs = re.findall(r"made (\d+) requests and got (\d+) responses", result.stdout)
    ok &= check("setup: two report lines", len(pairs) == 2, result.stdout)
    ok &= check(
        # The stop can strand in flight requests, C shows the same
        # gaps of one or two, so only the direction is fixed.
        "setup: responses never exceed requests",
        pairs and all(int(made) >= int(got) and int(got) > 0 for made, got in pairs),
        pairs,
    )
    if not ok:
        failures += 1

    cases = [
        ("addr.lua", {}),
        ("auth.lua", {}),
        ("counter.lua", {}),
        ("delay.lua", {}),
        ("pipeline.lua", {}),
        ("post.lua", {}),
        ("report.lua", {}),
    ]
    for script, _ in cases:
        name = script.removesuffix(".lua")
        with seen_lock:
            for key in seen:
                seen[key].clear()
        result, _ = run(WRKRS, port, f"scripts/{script}")
        ok = check(f"{name}: exit zero", result.returncode == 0, result.stderr[-200:])
        ok &= check(f"{name}: completed requests", wrkrs_requests(result) > 0, result.stderr[-200:])

        if script == "addr.lua":
            ok &= check(
                "addr: init printed the thread address",
                "thread addr:" in result.stdout,
                result.stdout,
            )
        if script == "auth.lua":
            ok &= check("auth: hit /authenticate", "/authenticate" in seen["paths"])
            ok &= check("auth: switched to /resource", "/resource" in seen["paths"])
            ok &= check("auth: token header echoed", "abc123" in seen["tokens"])
        if script == "counter.lua":
            ok &= check("counter: header values seen", len(seen["counters"]) > 3)
            # The thread zero pipeline probe consumes one request()
            # call, so thread zero sends 1, 2, 3 first, in C too.
            ok &= check(
                "counter: header starts at one",
                seen["counters"][:3] == [1, 2, 3],
                seen["counters"][:5],
            )
            ok &= check("counter: paths carry the counter", "/1" in seen["paths"])
        if script == "delay.lua":
            requests = wrkrs_requests(result)
            # A 10-50 ms delay per request bounds one connection
            # well below the full rate.
            ok &= check("delay: rate bounded by the delay", 3 <= requests < 300, requests)
        if script == "pipeline.lua":
            for query in ("/?foo", "/?bar", "/?baz"):
                ok &= check(f"pipeline: saw {query}", query in seen["paths"])
        if script == "post.lua":
            ok &= check(
                "post: body and content type",
                "foo=bar&baz=quux" in seen["posts"],
                seen["posts"][:2],
            )
        if script == "report.lua":
            for percentile in ("50%", "90%", "99%", "99.999%"):
                ok &= check(
                    f"report: printed {percentile}",
                    any(line.startswith(percentile) for line in result.stdout.splitlines()),
                    result.stdout.splitlines()[-4:],
                )
        if not ok:
            failures += 1

    # stop.lua stops the thread loop at the next tick but the main
    # thread still waits out the duration, the C behavior.
    with seen_lock:
        seen["paths"].clear()
    result, elapsed = run(WRKRS, port, "scripts/stop.lua", extra=("-d2s",))
    requests = wrkrs_requests(result)
    ok = check("stop: exit zero", result.returncode == 0)
    # The 100th response fires the stop, the loop exits at the end of
    # that iteration, and one request is in flight, so exactly 100.
    ok &= check("stop: exactly one hundred responses", requests == 100, requests)
    ok &= check("stop: main waited the full duration", elapsed >= 1.9, elapsed)
    if not ok:
        failures += 1

    # The same stop and delay shapes under the C binary, when one
    # is present for comparison.
    if os.path.exists(WRK):
        c_result, c_elapsed = run(WRK, port, "scripts/stop.lua", extra=("-d2s",))
        c_requests = wrk_requests(c_result)
        ok = check("stop: C also stops at one hundred", c_requests == 100, c_requests)
        ok &= check("stop: C waited the full duration", c_elapsed >= 1.9, c_elapsed)
        c_result, _ = run(WRK, port, "scripts/delay.lua")
        ok &= check(
            "delay: C rate bounded the same",
            3 <= wrk_requests(c_result) < 300,
            wrk_requests(c_result),
        )
        with seen_lock:
            seen["counters"].clear()
            seen["paths"].clear()
        c_result, _ = run(WRK, port, "scripts/counter.lua")
        ok &= check(
            "counter: C also starts at one",
            seen["counters"][:3] == [1, 2, 3],
            seen["counters"][:5],
        )
        if not ok:
            failures += 1
    else:
        print("  note: no C binary, comparison legs skipped")

    # The JavaScript twins under QuickJS carry the same observable
    # behavior as their Lua counterparts.
    for script in ("counter.js", "delay.js", "pipeline.js", "post.js", "report.js"):
        name = script.removesuffix(".js")
        with seen_lock:
            for key in seen:
                seen[key].clear()
        result, _ = run(WRKRS, port, f"scripts/{script}")
        ok = check(f"{name}.js: exit zero", result.returncode == 0, result.stderr[-300:])
        ok &= check(f"{name}.js: completed requests", wrkrs_requests(result) > 0, result.stderr[-300:])
        if script == "counter.js":
            ok &= check("counter.js: header starts at one", seen["counters"][:3] == [1, 2, 3], seen["counters"][:5])
        if script == "delay.js":
            ok &= check("delay.js: rate bounded", 3 <= wrkrs_requests(result) < 300, wrkrs_requests(result))
        if script == "pipeline.js":
            for query in ("/?foo", "/?bar", "/?baz"):
                ok &= check(f"pipeline.js: saw {query}", query in seen["paths"])
        if script == "post.js":
            ok &= check("post.js: body and content type", "foo=bar&baz=quux" in seen["posts"], seen["posts"][:2])
        if script == "report.js":
            for percentile in ("50%", "90%", "99%", "99.999%"):
                ok &= check(
                    f"report.js: printed {percentile}",
                    any(line.startswith(percentile) for line in result.stdout.splitlines()),
                    result.stdout.splitlines()[-4:],
                )
        if not ok:
            failures += 1

    # The stop twin under QuickJS.
    result, elapsed = run(WRKRS, port, "scripts/stop.js", extra=("-d2s",))
    requests = wrkrs_requests(result)
    ok = check("stop.js: exit zero", result.returncode == 0, result.stderr[-300:])
    ok &= check("stop.js: exactly one hundred responses", requests == 100, requests)
    ok &= check("stop.js: main waited the full duration", elapsed >= 1.9, elapsed)
    if not ok:
        failures += 1

    # Benchmark files drive the config engine, TOML and JSON shape
    # the same POST as post.lua and a stop file lands on exactly one
    # hundred like stop.lua.
    for bench in ("bench.toml", "bench.json"):
        with seen_lock:
            for key in seen:
                seen[key].clear()
        result, _ = run(WRKRS, port, f"scripts/{bench}")
        ok = check(f"{bench}: exit zero", result.returncode == 0, result.stderr[-200:])
        ok &= check(f"{bench}: completed requests", wrkrs_requests(result) > 0, result.stderr[-200:])
        ok &= check(
            f"{name}: body and content type",
            "foo=bar&baz=quux" in seen["posts"],
            seen["posts"][:2],
        )
        if not ok:
            failures += 1

    with seen_lock:
        seen["paths"].clear()
    result, elapsed = run(WRKRS, port, "scripts/stop.toml", extra=("-d2s",))
    requests = wrkrs_requests(result)
    ok = check("stop.toml: exit zero", result.returncode == 0)
    ok &= check("stop.toml: exactly one hundred responses", requests == 100, requests)
    ok &= check("stop.toml: main waited the full duration", elapsed >= 1.9, elapsed)
    if not ok:
        failures += 1

    # A broken benchmark file fails the run with the file named in
    # the error, data files do not fall back to defaults.
    broken = os.path.join(tempfile.gettempdir(), "wrkrs-e2e-broken.toml")
    with open(broken, "w") as handle:
        handle.write("[request]\nnope = 1\n")
    result = subprocess.run(
        [WRKRS, "-t1", "-c1", "-d1s", "-s", broken, f"http://127.0.0.1:{port}/"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    ok = check("broken.toml: exit one", result.returncode == 1, result.returncode)
    ok &= check(
        "broken.toml: error names the file",
        "wrkrs-e2e-broken.toml" in result.stderr and "TOML parse error" in result.stderr,
        result.stderr[-200:],
    )
    if not ok:
        failures += 1

    # A dying target surfaces the same socket error line in both
    # binaries: the acceptor passes the resolve probe but closes
    # every connection at once, so the run collects read errors in
    # the eager reconnect storm.
    dying = socket.socket()
    dying.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    dying.bind(("127.0.0.1", 0))
    dying.listen(128)
    dying_port = dying.getsockname()[1]

    def kill_all():
        while True:
            conn, _ = dying.accept()
            conn.close()

    threading.Thread(target=kill_all, daemon=True).start()

    error_lines = []
    for name, binary in (("wrkrs", WRKRS), ("C", WRK)):
        if name == "C" and not os.path.exists(WRK):
            continue
        result = subprocess.run(
            [binary, "-t1", "-c1", "-d1s", f"http://127.0.0.1:{dying_port}/"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        match = re.search(r"  Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)", result.stdout)
        ok = check(f"errors: {name} prints the socket error line", match is not None, result.stdout[-200:])
        if match:
            ok &= check(f"errors: {name} counted the read storm", int(match.group(2)) > 0, match.group(0))
            error_lines.append("connect N, read N, write N, timeout N")
        if not ok:
            failures += 1
    check("errors: the line shape matches", len(set(error_lines)) <= 1, error_lines)

    print("all scripts ok" if failures == 0 else f"{failures} script cases failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
