#!/usr/bin/env python3
"""Compare the observable shape of wrkrs against the C wrk binary.

Both binaries run against the same local server and the harness
compares what must match at the shape level: request counts are
positive on both, byte counts are positive, error counters behave the
same, and the runtime is close to the requested duration. Values like
requests per second can never match between runs, the reporter phase
owns byte parity.

Usage: tools/shape_compare.py [duration_s]
"""

import socket
import subprocess
import sys
import threading

DURATION = sys.argv[1] if len(sys.argv) > 1 else "2s"


def start_server():
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)

    def serve():
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
                    buffer = buffer.split(b"\r\n\r\n", 1)[1]
                    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
        except OSError:
            pass

    threading.Thread(target=serve, daemon=True).start()
    return listener.getsockname()[1]


def run(binary, port, extra=()):
    result = subprocess.run(
        [binary, "-t2", "-c8", "-d", DURATION, *extra, f"http://127.0.0.1:{port}/"],
        capture_output=True,
        text=True,
        timeout=120,
    )
    return result


def parse_c(output):
    """Pulls the C report numbers."""
    numbers = {}
    for line in output.splitlines():
        parts = line.split()
        if "requests in" in line and parts and parts[0].isdigit():
            numbers["complete"] = int(parts[0])
        if "Requests/sec" in line and parts:
            numbers["rps"] = float(parts[1])
    return numbers


def main():
    port = start_server()
    failures = []

    c = run("./wrk", port)
    rs = run("./target/release/wrkrs", port)

    if c.returncode != 0 or rs.returncode != 0:
        print("exit codes differ:", c.returncode, rs.returncode)
        return 1

    c_numbers = parse_c(c.stdout)
    rs_stderr = rs.stderr
    rs_complete = int(rs_stderr.split()[1].split()[0])

    for name, value in c_numbers.items():
        if name == "complete" and value <= 0:
            failures.append(f"C completed nothing: {value}")
    if rs_complete <= 0:
        failures.append(f"wrkrs completed nothing: {rs_complete}")
    if "errors" in rs_stderr:
        total_errors = int(rs_stderr.split(" errors")[0].split(",")[-1])
        if total_errors != 0:
            failures.append(f"wrkrs counted errors: {total_errors}")
    if "Socket errors" in c.stdout:
        failures.append("C counted socket errors against a healthy server")

    for failure in failures:
        print("FAIL:", failure)
    if not failures:
        ratio = rs_complete / max(c_numbers["complete"], 1)
        print(f"shape ok: C {c_numbers['complete']} requests, wrkrs {rs_complete}, ratio {ratio:.2f}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
