#!/usr/bin/env python3
"""Run the https path end to end against a self-signed server.

A python TLS server serves keep-alive responses with a certificate
generated on the fly. Both binaries run against it, neither verifies
the certificate, and the harness checks the completed requests and
the absence of socket errors.

Usage: tools/https_e2e.py [duration_s]
"""

import os
import re
import ssl
import subprocess
import sys
import tempfile
import threading

DURATION = sys.argv[1] if len(sys.argv) > 1 else "2s"


def certificate(directory):
    """Generates a self signed certificate with openssl."""
    cert = os.path.join(directory, "cert.pem")
    key = os.path.join(directory, "key.pem")
    subprocess.run(
        [
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", key, "-out", cert, "-days", "2",
            "-subj", "/CN=localhost",
        ],
        check=True,
        capture_output=True,
    )
    return cert, key


def start_server(cert, key):
    listener = __import__("socket").socket()
    listener.setsockopt(__import__("socket").SOL_SOCKET, __import__("socket").SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(64)

    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)

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
            try:
                conn = context.wrap_socket(conn, server_side=True)
            except ssl.SSLError:
                continue
            threading.Thread(target=handle, args=(conn,), daemon=True).start()

    threading.Thread(target=accept_loop, daemon=True).start()
    return listener.getsockname()[1]


def run(binary, port):
    return subprocess.run(
        [binary, "-t1", "-c2", "-d", DURATION, f"https://127.0.0.1:{port}/"],
        capture_output=True,
        text=True,
        timeout=120,
    )


def main():
    with tempfile.TemporaryDirectory() as directory:
        cert, key = certificate(directory)
        port = start_server(cert, key)

        failures = []
        outputs = {}
        for name, binary in (("C", "./wrk"), ("wrkrs", "./target/release/wrkrs")):
            result = run(binary, port)
            outputs[name] = result.stdout
            if result.returncode != 0:
                failures.append(f"{name} exited {result.returncode}: {result.stderr[-200:]}")

        for name, out in outputs.items():
            match = re.search(r"  (\d+) requests in", out)
            requests = int(match.group(1)) if match else -1
            if requests <= 0:
                failures.append(f"{name} completed nothing: {out[-200:]}")
            if "Socket errors" in out:
                failures.append(f"{name} counted socket errors: {out[-300:]}")

        for failure in failures:
            print("FAIL:", failure)
        if not failures:
            c = re.search(r"  (\d+) requests in", outputs["C"]).group(1)
            rs = re.search(r"  (\d+) requests in", outputs["wrkrs"]).group(1)
            ratio = int(rs) / max(int(c), 1)
            print(f"https ok: C {c} requests, wrkrs {rs}, ratio {ratio:.2f}")
        return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
