#!/usr/bin/env python3
"""Performance sanity of wrkrs against the C binary.

Runs a spread of configurations against a local server, one pair of
runs per configuration, and prints the throughput ratio. Numbers
vary between runs, the check is that wrkrs stays near one.

Usage: tools/perf_sanity.py [duration_s]
"""

import os
import re
import socket
import ssl
import subprocess
import sys
import tempfile
import threading

DURATION = sys.argv[1] if len(sys.argv) > 1 else "2s"

CONFIGS = [
    ("plain t1c1", ["-t1", "-c1"], "http"),
    ("plain t2c8", ["-t2", "-c8"], "http"),
    ("plain t4c50", ["-t4", "-c50"], "http"),
    ("pipeline t2c8", ["-t2", "-c8", "-s", "scripts/pipeline.lua"], "http"),
    ("delay t2c8", ["-t2", "-c8", "-s", "scripts/delay.lua"], "http"),
    ("https t2c8", ["-t2", "-c8"], "https"),
]


def start_plain():
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)
    port = listener.getsockname()[1]
    serve(listener, None)
    return f"http://127.0.0.1:{port}/"


def start_tls():
    directory = tempfile.mkdtemp()
    cert = os.path.join(directory, "cert.pem")
    key = os.path.join(directory, "key.pem")
    subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
         "-keyout", key, "-out", cert, "-days", "2", "-subj", "/CN=localhost"],
        check=True, capture_output=True,
    )
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(128)
    port = listener.getsockname()[1]
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    serve(listener, context)
    return f"https://127.0.0.1:{port}/"


def serve(listener, context):
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
            if context:
                try:
                    conn = context.wrap_socket(conn, server_side=True)
                except ssl.SSLError:
                    continue
            threading.Thread(target=handle, args=(conn,), daemon=True).start()

    threading.Thread(target=accept_loop, daemon=True).start()


def requests(binary, extra, url):
    result = subprocess.run(
        [binary, *extra, "-d", DURATION, url],
        capture_output=True, text=True, timeout=300,
    )
    match = re.search(r"  (\d+) requests in", result.stdout)
    return int(match.group(1)) if match else -1


def main():
    http_url = start_plain()
    https_url = start_tls()
    ratios = []
    have_c = os.path.exists("./wrk")
    if not have_c:
        print("note: no C binary, reporting wrkrs alone")
    print(f"{'configuration':<18} {'C':>10} {'wrkrs':>10} {'ratio':>7}")
    for name, extra, scheme in CONFIGS:
        url = https_url if scheme == "https" else http_url
        c = requests("./wrk", extra, url) if have_c else 0
        rs = requests("./target/release/wrkrs", extra, url)
        ratio = rs / max(c, 1) if c else 0.0
        ratios.append(ratio)
        print(f"{name:<18} {c if c else '-':>10} {rs:>10} {ratio if c else 0.0:>7.2f}")
    if have_c:
        worst = min(ratios)
        print(f"worst ratio {worst:.2f}")
        return 0 if worst > 0.5 else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
