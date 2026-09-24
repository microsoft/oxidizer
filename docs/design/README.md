# Repository verification

This repository uses Cargo Anvil as the source of truth for Rust verification.
Use the generated Anvil recipes locally and the generated Anvil workflows in
CI. Do not maintain parallel implementations of Anvil checks.

Keep repository-specific automation only for capabilities outside Anvil's
scope, such as release tooling and tests for that tooling. Generated Anvil
files are updated with `cargo anvil`, not edited directly.

The handwritten `repository-checks.yml` also verifies the
`http_headers_simd` `no_std` contract with package-isolated unit and integration
tests on AArch64 and x86-family Linux. The generated Anvil test groups can select the
`http_headers` facade alongside its SIMD dependency, which re-enables the
dependency's `std` feature even in their `--no-default-features` leg. The
isolated jobs cover otherwise-unselected configurations rather than duplicating
an Anvil test group. They use Anvil's selected stable toolchain and participate
in the stable `Required repository checks` fan-in.

| Target | Host | Global CPU baseline |
| --- | --- | --- |
| `aarch64-unknown-linux-gnu` | Native AArch64 Linux | Target defaults |
| `x86_64-unknown-linux-gnu` | Native x86-64 Linux | `x86-64`, including SSE2 |
| `i686-unknown-linux-gnu` | x86-64 Linux, executing 32-bit binaries | `pentium4`, including SSE2 |
| `i586-unknown-linux-gnu` | x86-64 Linux, executing 32-bit binaries | `pentium`, without SSE2 |

`just test-http-headers-simd-no-std-arm` retains the native AArch64 host guard
and requires the no_std NEON detector and differential test names before
executing the suite. `just test-http-headers-simd-no-std-x86 --target TARGET`
requires an x86-64 Linux host, both no_std CPUID-cache tests, and the independent
runtime-feature oracle test. The 32-bit targets additionally require the
no_std SSE2 detector test; unlike x86-64, their SSE2 availability depends on the
compile-time feature. CI installs the corresponding Rust target and 32-bit
linker/runtime where needed.

The x86-family recipe replaces `CARGO_ENCODED_RUSTFLAGS` with the selected
`-Ctarget-cpu` and `-Ctarget-feature=-ssse3,-sse4.2`. Cargo gives these encoded
flags precedence over inherited `RUSTFLAGS`, target-specific flags, and build
flags, intentionally overriding the repository's `x86-64-v3` setting. Explicit
`--target` confines these flags to target compilation rather than host build
scripts and procedural macros. Required test names are checked before the
suite runs, so re-enabling `std` or either compile-time ISA feature cannot
silently omit its cache test and pass.

These jobs do not mask the host's actual CPUID capabilities: they exercise
cached runtime detection, not necessarily the result for a processor lacking
SSSE3 or SSE4.2. Their runtime scope is Linux, not Windows or other operating
systems. These are ordinary correctness tests, not an additional coverage or
mutation requirement for no_std-only code.

The `http_headers` and `http_headers_simd` crates are excluded from mutation
testing through `.cargo/mutants.toml`. Their ordinary tests, coverage checks,
and Miri checks remain enabled.

Under Miri, `http_headers` samples repetitive generated inputs and byte
substitutions while retaining fixed regressions and numeric and vector
boundaries. Large WebSocket admission-limit cases retain the actual byte,
line, and item limits and both ownership paths, without repeating every
source representation and decoding mode. Native test runs keep the full
input corpora and source/mode combinations.

Credential zeroization checks use smaller heap-backed buffers under Miri,
preserving base64 padding shapes, allocation growth, reuse, and full wipe
assertions. Differential checks compare complete errors without repeatedly
formatting them.

The SIMD crate also reduces repeated byte-class combinations under Miri:
public scanner checks retain every byte in every lane at 33 bytes, and
byte-class boundaries at every other tested length. Direct architecture
checks keep their full byte/lane matrices. Range grammar enumeration keeps
short inputs and two-specification forms; fixed longer regressions remain.
Allocation-counter concurrency tests keep all four threads with fewer
updates per thread. Native runs retain the exhaustive input corpora.

Bolero-generated properties run natively, but are ignored under Miri because
Bolero's corpus replay requires filesystem access blocked by Miri isolation.
Fixed regression seeds and deterministic scanner and parser checks still run
under Miri. The in-memory Axum example tests use a runtime without an OS I/O
driver, so they do not require Windows I/O completion port emulation.
