# Performance

This table is the comparative snapshot imported with `http_headers` 0.1.0. It
is not a promise of current timing on different hardware or dependency
versions; use the repository's metabench targets to measure the current
checkout.

Each cell reports metabench's median wall-clock time, Callgrind instruction
count, allocation count, and total allocated bytes for one typed decode and
read. Benchmarks consume the decoded value and force comparable semantic work
when `headers 0.4.1` defers parsing. Setup uses a prebuilt `HeaderMap` outside
the measured operation. Criterion uses a 1 s warm-up, 3 s measurement period,
and 60 samples per arm. Each cell is formatted as time, instructions, then
allocation count / allocated bytes. `n/a` means `headers 0.4.1` does not
provide that typed header.

| Header | `headers 0.4.1` | `http_headers` (owned) | `http_headers` (borrowed) |
|---|---:|---:|---:|
| Accept | n/a | 414.4 ns<br>1,387 instr<br>1 allocs / 24 B | 353.1 ns<br>1,055 instr<br>0 allocs / 0 B |
| Accept-Encoding | n/a | 140.4 ns<br>679 instr<br>0 allocs / 0 B | 86.4 ns<br>449 instr<br>0 allocs / 0 B |
| Accept-Language | n/a | 165.9 ns<br>731 instr<br>0 allocs / 0 B | 109.5 ns<br>502 instr<br>0 allocs / 0 B |
| Accept-Ranges | 61.9 ns<br>590 instr<br>1 allocs / 24 B | 71.6 ns<br>408 instr<br>0 allocs / 0 B | 67.7 ns<br>369 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Credentials | 30.1 ns<br>224 instr<br>0 allocs / 0 B | 38.5 ns<br>227 instr<br>0 allocs / 0 B | 38.3 ns<br>227 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Headers | 230.8 ns<br>2,021 instr<br>2 allocs / 36 B | 143.1 ns<br>914 instr<br>0 allocs / 0 B | 133.1 ns<br>791 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Methods | 101.0 ns<br>1,185 instr<br>1 allocs / 24 B | 133.9 ns<br>725 instr<br>0 allocs / 0 B | 55.6 ns<br>529 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Origin | 142.1 ns<br>1,335 instr<br>2 allocs / 43 B | 122.6 ns<br>670 instr<br>0 allocs / 0 B | 58.5 ns<br>555 instr<br>0 allocs / 0 B |
| Access-Control-Expose-Headers | 152.7 ns<br>1,816 instr<br>2 allocs / 36 B | 126.6 ns<br>827 instr<br>0 allocs / 0 B | 81.7 ns<br>704 instr<br>0 allocs / 0 B |
| Access-Control-Max-Age | 39.0 ns<br>325 instr<br>0 allocs / 0 B | 24.7 ns<br>291 instr<br>0 allocs / 0 B | 36.3 ns<br>291 instr<br>0 allocs / 0 B |
| Access-Control-Request-Headers | 146.2 ns<br>1,982 instr<br>2 allocs / 36 B | 144.1 ns<br>914 instr<br>0 allocs / 0 B | 92.6 ns<br>791 instr<br>0 allocs / 0 B |
| Access-Control-Request-Method | 30.1 ns<br>269 instr<br>0 allocs / 0 B | 41.1 ns<br>270 instr<br>0 allocs / 0 B | 23.8 ns<br>255 instr<br>0 allocs / 0 B |
| Allow | 104.5 ns<br>1,190 instr<br>1 allocs / 24 B | 105.4 ns<br>645 instr<br>0 allocs / 0 B | 41.9 ns<br>440 instr<br>0 allocs / 0 B |
| Authorization (Basic) | 129.6 ns<br>995 instr<br>1 allocs / 18 B | 108.6 ns<br>654 instr<br>0 allocs / 0 B | 61.3 ns<br>539 instr<br>0 allocs / 0 B |
| Authorization (Bearer) | 51.3 ns<br>546 instr<br>1 allocs / 24 B | 95.9 ns<br>702 instr<br>1 allocs / 24 B | 61.3 ns<br>489 instr<br>0 allocs / 0 B |
| Cache-Control | 161.2 ns<br>1,338 instr<br>0 allocs / 0 B | 131.7 ns<br>1,063 instr<br>0 allocs / 0 B | 72.9 ns<br>841 instr<br>0 allocs / 0 B |
| Content-Length | 46.9 ns<br>367 instr<br>0 allocs / 0 B | 34.7 ns<br>333 instr<br>0 allocs / 0 B | 30.8 ns<br>333 instr<br>0 allocs / 0 B |
| Content-Range | 102.3 ns<br>1,019 instr<br>0 allocs / 0 B | 125.5 ns<br>761 instr<br>0 allocs / 0 B | 78.2 ns<br>619 instr<br>0 allocs / 0 B |
| Content-Security-Policy | n/a | 91.6 ns<br>576 instr<br>1 allocs / 24 B | 25.6 ns<br>223 instr<br>0 allocs / 0 B |
| Content-Type | 138.6 ns<br>1,606 instr<br>1 allocs / 31 B | 105.2 ns<br>440 instr<br>0 allocs / 0 B | 34.3 ns<br>285 instr<br>0 allocs / 0 B |
| ETag | 38.7 ns<br>554 instr<br>1 allocs / 24 B | 98.2 ns<br>472 instr<br>0 allocs / 0 B | 38.9 ns<br>346 instr<br>0 allocs / 0 B |
| Host | 90.6 ns<br>1,021 instr<br>2 allocs / 40 B | 145.4 ns<br>548 instr<br>0 allocs / 0 B | 99.0 ns<br>527 instr<br>0 allocs / 0 B |
| If-Match | 124.7 ns<br>1,664 instr<br>2 allocs / 49 B | 121.1 ns<br>890 instr<br>0 allocs / 0 B | 83.1 ns<br>707 instr<br>0 allocs / 0 B |
| If-Modified-Since | 100.2 ns<br>1,007 instr<br>0 allocs / 0 B | 102.0 ns<br>625 instr<br>0 allocs / 0 B | 58.7 ns<br>528 instr<br>0 allocs / 0 B |
| If-None-Match | 120.2 ns<br>1,690 instr<br>2 allocs / 49 B | 121.8 ns<br>867 instr<br>0 allocs / 0 B | 100.0 ns<br>713 instr<br>0 allocs / 0 B |
| If-Range | 61.8 ns<br>567 instr<br>1 allocs / 24 B | 98.3 ns<br>452 instr<br>0 allocs / 0 B | 49.3 ns<br>354 instr<br>0 allocs / 0 B |
| If-Unmodified-Since | 127.0 ns<br>1,007 instr<br>0 allocs / 0 B | 116.9 ns<br>625 instr<br>0 allocs / 0 B | 67.2 ns<br>528 instr<br>0 allocs / 0 B |
| Last-Modified | 112.5 ns<br>1,007 instr<br>0 allocs / 0 B | 106.6 ns<br>625 instr<br>0 allocs / 0 B | 65.0 ns<br>528 instr<br>0 allocs / 0 B |
| Location | 42.4 ns<br>352 instr<br>1 allocs / 24 B | 129.4 ns<br>822 instr<br>1 allocs / 24 B | 74.3 ns<br>595 instr<br>0 allocs / 0 B |
| Range | 45.0 ns<br>553 instr<br>1 allocs / 24 B | 100.5 ns<br>492 instr<br>0 allocs / 0 B | 56.6 ns<br>419 instr<br>0 allocs / 0 B |
| Referrer-Policy | 103.7 ns<br>839 instr<br>0 allocs / 0 B | 99.4 ns<br>556 instr<br>0 allocs / 0 B | 38.6 ns<br>301 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Accept | 40.9 ns<br>352 instr<br>1 allocs / 24 B | 100.4 ns<br>423 instr<br>0 allocs / 0 B | 36.5 ns<br>301 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Extensions | n/a | 111.1 ns<br>575 instr<br>0 allocs / 0 B | 44.9 ns<br>311 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Key | 39.6 ns<br>488 instr<br>1 allocs / 24 B | 99.5 ns<br>423 instr<br>0 allocs / 0 B | 43.1 ns<br>301 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Protocol | n/a | 97.0 ns<br>536 instr<br>0 allocs / 0 B | 43.6 ns<br>300 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Version | 29.3 ns<br>226 instr<br>0 allocs / 0 B | 28.9 ns<br>234 instr<br>0 allocs / 0 B | 29.1 ns<br>234 instr<br>0 allocs / 0 B |
| Server | 62.9 ns<br>627 instr<br>1 allocs / 24 B | 90.9 ns<br>418 instr<br>0 allocs / 0 B | 32.4 ns<br>291 instr<br>0 allocs / 0 B |
| Set-Cookie | 63.3 ns<br>666 instr<br>2 allocs / 184 B | 117.0 ns<br>657 instr<br>1 allocs / 24 B | 52.2 ns<br>315 instr<br>0 allocs / 0 B |
| Strict-Transport-Security | 180.5 ns<br>1,434 instr<br>0 allocs / 0 B | 96.5 ns<br>436 instr<br>0 allocs / 0 B | 50.0 ns<br>380 instr<br>0 allocs / 0 B |
| User-Agent | 67.1 ns<br>610 instr<br>1 allocs / 24 B | 109.0 ns<br>526 instr<br>1 allocs / 24 B | 38.3 ns<br>291 instr<br>0 allocs / 0 B |
| Vary | 129.9 ns<br>1,370 instr<br>1 allocs / 24 B | 115.0 ns<br>737 instr<br>0 allocs / 0 B | 59.6 ns<br>546 instr<br>0 allocs / 0 B |
| X-Content-Type-Options | n/a | 89.1 ns<br>403 instr<br>0 allocs / 0 B | 40.7 ns<br>295 instr<br>0 allocs / 0 B |
