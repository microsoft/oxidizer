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
| Accept | n/a | 411.9 ns<br>1,454 instr<br>1 allocs / 24 B | 332.2 ns<br>1,088 instr<br>0 allocs / 0 B |
| Accept-Encoding | n/a | 130.7 ns<br>746 instr<br>0 allocs / 0 B | 74.0 ns<br>482 instr<br>0 allocs / 0 B |
| Accept-Language | n/a | 162.3 ns<br>798 instr<br>0 allocs / 0 B | 101.8 ns<br>535 instr<br>0 allocs / 0 B |
| Accept-Ranges | 48.9 ns<br>590 instr<br>1 allocs / 24 B | 92.5 ns<br>597 instr<br>0 allocs / 0 B | 56.3 ns<br>407 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Credentials | 23.4 ns<br>224 instr<br>0 allocs / 0 B | 39.8 ns<br>322 instr<br>0 allocs / 0 B | 32.2 ns<br>322 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Headers | 178.3 ns<br>2,021 instr<br>2 allocs / 36 B | 124.7 ns<br>978 instr<br>0 allocs / 0 B | 90.6 ns<br>911 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Methods | 111.5 ns<br>1,185 instr<br>1 allocs / 24 B | 139.8 ns<br>799 instr<br>0 allocs / 0 B | 81.0 ns<br>686 instr<br>0 allocs / 0 B |
| Access-Control-Allow-Origin | 131.1 ns<br>1,335 instr<br>2 allocs / 43 B | 111.9 ns<br>702 instr<br>0 allocs / 0 B | 56.6 ns<br>623 instr<br>0 allocs / 0 B |
| Access-Control-Expose-Headers | 114.6 ns<br>1,816 instr<br>2 allocs / 36 B | 122.1 ns<br>891 instr<br>0 allocs / 0 B | 68.6 ns<br>824 instr<br>0 allocs / 0 B |
| Access-Control-Max-Age | 27.0 ns<br>325 instr<br>0 allocs / 0 B | 33.9 ns<br>358 instr<br>0 allocs / 0 B | 31.3 ns<br>358 instr<br>0 allocs / 0 B |
| Access-Control-Request-Headers | 166.8 ns<br>1,982 instr<br>2 allocs / 36 B | 125.8 ns<br>978 instr<br>0 allocs / 0 B | 94.7 ns<br>911 instr<br>0 allocs / 0 B |
| Access-Control-Request-Method | 32.5 ns<br>269 instr<br>0 allocs / 0 B | 33.2 ns<br>298 instr<br>0 allocs / 0 B | 33.9 ns<br>294 instr<br>0 allocs / 0 B |
| Allow | 83.3 ns<br>1,190 instr<br>1 allocs / 24 B | 109.0 ns<br>712 instr<br>0 allocs / 0 B | 51.5 ns<br>476 instr<br>0 allocs / 0 B |
| Authorization (Basic) | 80.6 ns<br>995 instr<br>1 allocs / 18 B | 102.6 ns<br>692 instr<br>0 allocs / 0 B | 74.4 ns<br>609 instr<br>0 allocs / 0 B |
| Authorization (Bearer) | 42.9 ns<br>546 instr<br>1 allocs / 24 B | 98.3 ns<br>739 instr<br>1 allocs / 24 B | 52.8 ns<br>552 instr<br>0 allocs / 0 B |
| Cache-Control | 108.8 ns<br>1,338 instr<br>0 allocs / 0 B | 117.9 ns<br>1,109 instr<br>0 allocs / 0 B | 79.9 ns<br>892 instr<br>0 allocs / 0 B |
| Content-Length | 28.5 ns<br>367 instr<br>0 allocs / 0 B | 33.6 ns<br>375 instr<br>0 allocs / 0 B | 33.3 ns<br>375 instr<br>0 allocs / 0 B |
| Content-Range | 75.3 ns<br>1,019 instr<br>0 allocs / 0 B | 108.8 ns<br>798 instr<br>0 allocs / 0 B | 64.8 ns<br>684 instr<br>0 allocs / 0 B |
| Content-Security-Policy | n/a | 94.1 ns<br>623 instr<br>1 allocs / 24 B | 23.2 ns<br>257 instr<br>0 allocs / 0 B |
| Content-Type | 113.8 ns<br>1,606 instr<br>1 allocs / 31 B | 99.9 ns<br>462 instr<br>0 allocs / 0 B | 45.0 ns<br>364 instr<br>0 allocs / 0 B |
| ETag | 32.7 ns<br>554 instr<br>1 allocs / 24 B | 88.5 ns<br>494 instr<br>0 allocs / 0 B | 45.9 ns<br>398 instr<br>0 allocs / 0 B |
| Host | 64.2 ns<br>1,021 instr<br>2 allocs / 40 B | 126.3 ns<br>584 instr<br>0 allocs / 0 B | 74.2 ns<br>594 instr<br>0 allocs / 0 B |
| If-Match | 76.4 ns<br>1,664 instr<br>2 allocs / 49 B | 96.9 ns<br>966 instr<br>0 allocs / 0 B | 63.5 ns<br>760 instr<br>0 allocs / 0 B |
| If-Modified-Since | 88.2 ns<br>1,007 instr<br>0 allocs / 0 B | 95.8 ns<br>661 instr<br>0 allocs / 0 B | 56.2 ns<br>590 instr<br>0 allocs / 0 B |
| If-None-Match | 75.5 ns<br>1,690 instr<br>2 allocs / 49 B | 100.0 ns<br>948 instr<br>0 allocs / 0 B | 63.4 ns<br>764 instr<br>0 allocs / 0 B |
| If-Range | 34.0 ns<br>567 instr<br>1 allocs / 24 B | 82.1 ns<br>470 instr<br>0 allocs / 0 B | 46.6 ns<br>411 instr<br>0 allocs / 0 B |
| If-Unmodified-Since | 80.4 ns<br>1,007 instr<br>0 allocs / 0 B | 91.8 ns<br>661 instr<br>0 allocs / 0 B | 53.7 ns<br>590 instr<br>0 allocs / 0 B |
| Last-Modified | 79.8 ns<br>1,007 instr<br>0 allocs / 0 B | 91.7 ns<br>661 instr<br>0 allocs / 0 B | 51.7 ns<br>590 instr<br>0 allocs / 0 B |
| Location | 29.0 ns<br>352 instr<br>1 allocs / 24 B | 102.1 ns<br>859 instr<br>1 allocs / 24 B | 58.9 ns<br>648 instr<br>0 allocs / 0 B |
| Range | 32.2 ns<br>553 instr<br>1 allocs / 24 B | 91.7 ns<br>555 instr<br>0 allocs / 0 B | 51.8 ns<br>489 instr<br>0 allocs / 0 B |
| Referrer-Policy | 64.3 ns<br>839 instr<br>0 allocs / 0 B | 98.7 ns<br>607 instr<br>0 allocs / 0 B | 32.6 ns<br>327 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Accept | 29.0 ns<br>352 instr<br>1 allocs / 24 B | 89.1 ns<br>461 instr<br>0 allocs / 0 B | 44.0 ns<br>369 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Extensions | n/a | 109.7 ns<br>723 instr<br>0 allocs / 0 B | 47.7 ns<br>442 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Key | 28.6 ns<br>488 instr<br>1 allocs / 24 B | 87.9 ns<br>463 instr<br>0 allocs / 0 B | 43.4 ns<br>371 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Protocol | n/a | 94.5 ns<br>611 instr<br>0 allocs / 0 B | 35.4 ns<br>354 instr<br>0 allocs / 0 B |
| Sec-WebSocket-Version | 19.0 ns<br>226 instr<br>0 allocs / 0 B | 42.7 ns<br>328 instr<br>0 allocs / 0 B | 42.8 ns<br>328 instr<br>0 allocs / 0 B |
| Server | 43.1 ns<br>627 instr<br>1 allocs / 24 B | 90.7 ns<br>456 instr<br>0 allocs / 0 B | 44.8 ns<br>358 instr<br>0 allocs / 0 B |
| Set-Cookie | 50.4 ns<br>666 instr<br>2 allocs / 184 B | 104.7 ns<br>690 instr<br>1 allocs / 24 B | 43.7 ns<br>352 instr<br>0 allocs / 0 B |
| Strict-Transport-Security | 137.6 ns<br>1,434 instr<br>0 allocs / 0 B | 92.0 ns<br>500 instr<br>0 allocs / 0 B | 62.3 ns<br>455 instr<br>0 allocs / 0 B |
| User-Agent | 53.2 ns<br>610 instr<br>1 allocs / 24 B | 110.0 ns<br>564 instr<br>1 allocs / 24 B | 44.1 ns<br>355 instr<br>0 allocs / 0 B |
| Vary | 110.3 ns<br>1,370 instr<br>1 allocs / 24 B | 109.0 ns<br>804 instr<br>0 allocs / 0 B | 53.9 ns<br>581 instr<br>0 allocs / 0 B |
| X-Content-Type-Options | n/a | 63.2 ns<br>440 instr<br>0 allocs / 0 B | 40.7 ns<br>360 instr<br>0 allocs / 0 B |
