# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

"""Controlled Linux receiver for the WinHTTP Nagle behavior experiment."""

import socket
import statistics
import time

TRIALS = 7
TIMEOUT_SECONDS = 3


def receive_exact(connection: socket.socket, size: int) -> bytes:
    received = bytearray()
    while len(received) < size:
        chunk = connection.recv(size - len(received))
        if not chunk:
            raise RuntimeError("connection closed before the expected data arrived")
        received.extend(chunk)
    return bytes(received)


def receive_http_headers(connection: socket.socket) -> None:
    tail = bytearray()
    while tail[-4:] != b"\r\n\r\n":
        tail.extend(receive_exact(connection, 1))
        if len(tail) > 64 * 1024:
            raise RuntimeError("HTTP headers exceeded 64 KiB")


def run_trial(listener: socket.socket, is_http: bool) -> float:
    connection, _ = listener.accept()
    with connection:
        connection.settimeout(TIMEOUT_SECONDS)
        if is_http:
            receive_http_headers(connection)

        # TCP_QUICKACK is a transient hint. Set it immediately before the measured receive pair.
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_QUICKACK, 0)
        receive_exact(connection, 1)
        first_at = time.monotonic_ns()
        receive_exact(connection, 1)
        elapsed_ms = (time.monotonic_ns() - first_at) / 1_000_000

        if is_http:
            connection.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
        else:
            connection.sendall(b"K")
        return elapsed_ms


def main() -> None:
    cases = (
        ("raw TCP, Nagle enabled", False),
        ("raw TCP, TCP_NODELAY", False),
        ("WinHTTP", True),
    )
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("0.0.0.0", 0))
        listener.listen()
        print(listener.getsockname()[1], flush=True)

        for label, is_http in cases:
            samples = [run_trial(listener, is_http) for _ in range(TRIALS)]
            print(
                f"{label}: median={statistics.median(samples):.3f} ms, "
                f"samples={[round(sample, 3) for sample in samples]}",
                flush=True,
            )


if __name__ == "__main__":
    main()
