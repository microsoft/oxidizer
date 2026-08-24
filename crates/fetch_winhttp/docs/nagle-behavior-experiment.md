# WinHTTP Nagle behavior experiment

This experiment determines whether WinHTTP exhibits Nagle's delayed-ACK stall for consecutive
small writes. WinHTTP does not expose its socket or report `TCP_NODELAY`, so the experiment measures
observable behavior rather than querying the option.

## Method

A Linux receiver runs under WSL2 so it is separated from the Windows loopback fast path. Before
each measured pair it sets `TCP_QUICKACK` to zero, allowing the Linux delayed-ACK policy to operate.
The Windows client waits for connection setup, writes one byte, waits 5 ms, and writes a second
byte. The receiver measures the interval between receiving the bytes.

Three fresh-connection cases run seven times:

1. a raw Windows TCP socket with Nagle explicitly enabled;
2. the same raw socket with `TCP_NODELAY`;
3. two synchronous `WinHttpWriteData` calls in a fixed-length HTTP/1.1 upload.

The raw cases calibrate the receiver and network path. The result is meaningful only if Nagle
produces a clear delayed-ACK stall while `TCP_NODELAY` preserves the intentional 5 ms spacing.

Run the receiver:

```text
wsl.exe -d Ubuntu-24.04 -- python3 \
  /mnt/d/repos/oxidizer-github/crates/fetch_winhttp/examples/nagle_receiver.py
```

Use the printed port and the WSL address from `wsl.exe hostname -I`:

```text
$env:NAGLE_RECEIVER = "<wsl-address>:<printed-port>"
cargo +1.93.0 run -p fetch_winhttp --example nagle_behavior
```

An attempted Windows-only receiver was not usable: the current host rejects
`SIO_TCP_SET_ACK_FREQUENCY` with `WSAEINVAL`, including on a routed interface. The retained probe
therefore requires a Linux receiver rather than silently testing on a path without controlled ACK
behavior.

## Observed result

```text
raw TCP, Nagle enabled: median=42.518 ms,
  samples=[38.762, 42.077, 42.518, 43.257, 40.410, 43.063, 42.961]
raw TCP, TCP_NODELAY: median=4.642 ms,
  samples=[5.230, 4.536, 4.532, 5.209, 4.642, 4.667, 4.561]
WinHTTP: median=5.354 ms,
  samples=[5.354, 5.493, 5.019, 5.341, 4.769, 5.622, 5.493]
```

The calibration separates the policies by approximately 38 ms. WinHTTP tracks the
`TCP_NODELAY` control and not the Nagle control.

A complete repeat produced medians of 42.671 ms, 4.640 ms, and 5.506 ms respectively, confirming
the separation.

The approximately 5 ms interval also shows that WinHTTP did not retain the first byte and coalesce
both writes: in that case the receiver would observe the bytes together rather than at the
intentional spacing. Under this HTTP/1.1 upload scenario, WinHTTP sent the second small write while
the first remained unacknowledged.

## Conclusion and limits

WinHTTP behaves as though Nagle is disabled for this connection on the tested Windows host. The
experiment establishes the absence of a Nagle/delayed-ACK stall; it does not prove whether WinHTTP
called `setsockopt(TCP_NODELAY)` or established equivalent behavior through an internal mechanism.

This is not a documented WinHTTP contract. The result may vary by Windows version, HTTP protocol,
TLS, proxy path, or internal connection implementation. Retaining a backend integration benchmark
can detect behavior changes, but a library cannot require `TCP_NODELAY` through the supported
WinHTTP API.
