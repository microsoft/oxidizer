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
| Accept | n/a | 399.0 ns<br>1,387 instr<br>1 allocs / 24 B | 343.5 ns<br>1,055 instr<br>0 allocs / 0 B |
| Accept-Encoding | n/a | 119.1 ns<br>679 instr<br>0 allocs / 0 B | 74.0 ns<br>449 instr<br>0 allocs / 0 B |
| Accept-Language | n/a | 148.1 ns<br>731 instr<br>0 allocs / 0 B | 104.7 ns<br>502 instr<br>0 allocs / 0 B |
| Accept-Ranges | 37.5 ns<br>590 instr<br>1 allocs / 24 B | 70.9 ns<br>408 instr<br>0 allocs / 0 B | 57.2 ns<br>369 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Credentials | 20.3 ns<br>224 instr<br>0 allocs / 0 B | 28.1 ns<br>227 instr<br>0 allocs / 0 B | 23.9 ns<br>227 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Headers | 195.6 ns<br>2,021 instr<br>2 allocs / 36 B | 149.9 ns<br>914 instr<br>0 allocs / 0 B | 82.8 ns<br>791 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Methods | 85.4 ns<br>1,185 instr<br>1 allocs / 24 B | 129.5 ns<br>725 instr<br>0 allocs / 0 B | 88.5 ns<br>529 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Origin | 192.1 ns<br>1,335 instr<br>2 allocs / 43 B | 136.5 ns<br>694 instr<br>0 allocs / 0 B | 54.6 ns<br>583 instr<br>0 allocs / 0 B |
| Access-Control-Expose-Headers | 133.6 ns<br>1,816 instr<br>2 allocs / 36 B | 114.7 ns<br>827 instr<br>0 allocs / 0 B | 93.1 ns<br>704 instr<br>0 allocs / 0 B |
| Access-Control-Max-Age | 27.8 ns<br>325 instr<br>0 allocs / 0 B | 24.0 ns<br>291 instr<br>0 allocs / 0 B | 29.4 ns<br>291 instr<br>0 allocs / 0 B |
| Access-Control-Request-Headers | 159.0 ns<br>1,982 instr<br>2 allocs / 36 B | 122.1 ns<br>914 instr<br>0 allocs / 0 B | 68.8 ns<br>791 instr<br>0 allocs / 0 B |
| Access-Control-Request-Method | 22.6 ns<br>269 instr<br>0 allocs / 0 B | 28.3 ns<br>270 instr<br>0 allocs / 0 B | 21.1 ns<br>255 instr<br>0 allocs / 0 B |
| Allow | 76.1 ns<br>1,190 instr<br>1 allocs / 24 B | 110.5 ns<br>645 instr<br>0 allocs / 0 B | 107.0 ns<br>440 instr<br>0 allocs / 0 B |
| Authorization (Basic) | 148.6 ns<br>995 instr<br>1 allocs / 18 B | 160.5 ns<br>654 instr<br>0 allocs / 0 B | 112.1 ns<br>539 instr<br>0 allocs / 0 B |
| Authorization (Bearer) | 84.4 ns<br>546 instr<br>1 allocs / 24 B | 211.7 ns<br>702 instr<br>1 allocs / 24 B | 91.3 ns<br>489 instr<br>0 allocs / 0 B |
| Cache-Control | 261.2 ns<br>1,338 instr<br>0 allocs / 0 B | 214.8 ns<br>1,060 instr<br>0 allocs / 0 B | 131.5 ns<br>838 instr<br>0 allocs / 0 B |
| Content-Length | 56.5 ns<br>367 instr<br>0 allocs / 0 B | 47.1 ns<br>333 instr<br>0 allocs / 0 B | 50.0 ns<br>333 instr<br>0 allocs / 0 B |
| Content-Range | 142.7 ns<br>1,019 instr<br>0 allocs / 0 B | 140.1 ns<br>741 instr<br>0 allocs / 0 B | 99.1 ns<br>619 instr<br>0 allocs / 0 B |
| Content-Security-Policy | n/a | 96.1 ns<br>576 instr<br>1 allocs / 24 B | 21.8 ns<br>223 instr<br>0 allocs / 0 B |
| Content-Type | 199.6 ns<br>1,606 instr<br>1 allocs / 31 B | 118.2 ns<br>438 instr<br>0 allocs / 0 B | 42.9 ns<br>283 instr<br>0 allocs / 0 B |
| ETag | 58.9 ns<br>554 instr<br>1 allocs / 24 B | 116.0 ns<br>472 instr<br>0 allocs / 0 B | 52.2 ns<br>346 instr<br>0 allocs / 0 B |
| Host | 104.3 ns<br>1,021 instr<br>2 allocs / 40 B | 141.1 ns<br>708 instr<br>0 allocs / 0 B | 106.4 ns<br>683 instr<br>0 allocs / 0 B |
| If-Match | 148.6 ns<br>1,664 instr<br>2 allocs / 49 B | 166.2 ns<br>890 instr<br>0 allocs / 0 B | 115.3 ns<br>707 instr<br>0 allocs / 0 B |
| If-Modified-Since | 151.1 ns<br>1,007 instr<br>0 allocs / 0 B | 122.0 ns<br>620 instr<br>0 allocs / 0 B | 74.0 ns<br>523 instr<br>0 allocs / 0 B |
| If-None-Match | 155.0 ns<br>1,690 instr<br>2 allocs / 49 B | 119.1 ns<br>867 instr<br>0 allocs / 0 B | 87.0 ns<br>713 instr<br>0 allocs / 0 B |
| If-Range | 53.7 ns<br>567 instr<br>1 allocs / 24 B | 89.8 ns<br>452 instr<br>0 allocs / 0 B | 45.9 ns<br>354 instr<br>0 allocs / 0 B |
| If-Unmodified-Since | 113.6 ns<br>1,007 instr<br>0 allocs / 0 B | 112.4 ns<br>620 instr<br>0 allocs / 0 B | 66.3 ns<br>523 instr<br>0 allocs / 0 B |
| Last-Modified | 124.7 ns<br>1,007 instr<br>0 allocs / 0 B | 104.2 ns<br>620 instr<br>0 allocs / 0 B | 60.5 ns<br>523 instr<br>0 allocs / 0 B |
| Location | 42.2 ns<br>352 instr<br>1 allocs / 24 B | 172.6 ns<br>1,029 instr<br>1 allocs / 24 B | 97.0 ns<br>786 instr<br>0 allocs / 0 B |
| Range | 42.8 ns<br>553 instr<br>1 allocs / 24 B | 94.7 ns<br>492 instr<br>0 allocs / 0 B | 46.6 ns<br>419 instr<br>0 allocs / 0 B |
| Referrer-Policy | 68.5 ns<br>839 instr<br>0 allocs / 0 B | 110.5 ns<br>556 instr<br>0 allocs / 0 B | 35.5 ns<br>301 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Accept | 35.3 ns<br>352 instr<br>1 allocs / 24 B | 103.6 ns<br>423 instr<br>0 allocs / 0 B | 33.5 ns<br>301 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Extensions | n/a | 109.9 ns<br>575 instr<br>0 allocs / 0 B | 38.4 ns<br>311 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Key | 36.3 ns<br>488 instr<br>1 allocs / 24 B | 99.8 ns<br>423 instr<br>0 allocs / 0 B | 36.1 ns<br>301 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Protocol | n/a | 93.7 ns<br>537 instr<br>0 allocs / 0 B | 35.1 ns<br>300 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Version | 19.6 ns<br>226 instr<br>0 allocs / 0 B | 23.5 ns<br>234 instr<br>0 allocs / 0 B | 27.0 ns<br>234 instr<br>0 allocs / 0 B |
| Server | 47.0 ns<br>627 instr<br>1 allocs / 24 B | 99.2 ns<br>418 instr<br>0 allocs / 0 B | 30.7 ns<br>291 instr<br>0 allocs / 0 B |
| Set-Cookie | 44.9 ns<br>666 instr<br>2 allocs / 184 B | 113.9 ns<br>657 instr<br>1 allocs / 24 B | 45.1 ns<br>308 instr<br>0 allocs / 0 B |
| Strict-Transport-Security | 99.1 ns<br>1,434 instr<br>0 allocs / 0 B | 91.9 ns<br>436 instr<br>0 allocs / 0 B | 46.5 ns<br>380 instr<br>0 allocs / 0 B |
| User-Agent | 49.1 ns<br>610 instr<br>1 allocs / 24 B | 105.2 ns<br>526 instr<br>1 allocs / 24 B | 29.3 ns<br>291 instr<br>0 allocs / 0 B |
| Vary | 76.1 ns<br>1,370 instr<br>1 allocs / 24 B | 106.2 ns<br>737 instr<br>0 allocs / 0 B | 43.5 ns<br>546 instr<br>0 allocs / 0 B |
| X-Content-Type-Options | n/a | 73.2 ns<br>403 instr<br>0 allocs / 0 B | 33.3 ns<br>295 instr<br>0 allocs / 0 B |
