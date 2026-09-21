#!/usr/bin/env python3
"""Check the SIGINT behavior against the C binary.

Both binaries run longer than needed, the harness interrupts them
after a second, and the checks mirror the C contract: the process
exits promptly, the report still prints with the requests completed
until the interrupt, and no crash markers appear.
"""

import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time

FAILURES = []


def check(name, condition, detail=""):
    print(f"  {'ok' if condition else 'FAIL'}: {name}" + ("" if condition else f" {detail}"))
    if not condition:
        FAILURES.append(name)


def start_server():
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)

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

    def accept_loop():
        while True:
            conn, _ = listener.accept()
            threading.Thread(target=handle, args=(conn,), daemon=True).start()

    threading.Thread(target=accept_loop, daemon=True).start()
    return listener.getsockname()[1]


def interrupted_run(binary, port):
    process = subprocess.Popen(
        [binary, "-t2", "-c8", "-d8s", f"http://127.0.0.1:{port}/"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    time.sleep(1.2)
    process.send_signal(signal.SIGINT)
    stdout, stderr = process.communicate(timeout=10)
    return process.returncode, stdout, stderr


def main():
    port = start_server()
    binaries = [("wrkrs", "./target/release/wrkrs")]
    if os.path.exists("./wrk"):
        binaries.append(("C", "./wrk"))
    else:
        print("  note: no C binary, the comparison leg is skipped")
    for name, binary in binaries:
        code, stdout, stderr = interrupted_run(binary, port)
        match = re.search(r"  (\d+) requests in ([0-9.]+)s", stdout)
        check(f"{name}: exit zero", code == 0, code)
        check(f"{name}: report printed", match is not None, stdout[-200:])
        if match:
            requests = int(match.group(1))
            runtime = float(match.group(2))
            check(f"{name}: requests completed", requests > 1000, requests)
            # The report reflects the moment of the interrupt, not
            # the full duration.
            check(f"{name}: stopped early", runtime < 5.0, runtime)
        check(f"{name}: no crash markers", "panicked" not in stderr and "BUG" not in stderr, stderr[-200:])

    print("sigint parity ok" if not FAILURES else f"{len(FAILURES)} checks failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
