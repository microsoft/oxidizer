# Repository verification

This repository uses Cargo Anvil as the source of truth for Rust verification.
Use the generated Anvil recipes locally and the generated Anvil workflows in
CI. Do not maintain parallel implementations of Anvil checks.

Keep repository-specific automation only for capabilities outside Anvil's
scope, such as release tooling and tests for that tooling. Generated Anvil
files are updated with `cargo anvil`, not edited directly.

## Upgrading Anvil

Anvil preserves files that differ from their last generated content.
An upgrade can therefore leave an older customized file in place.
The version in `.anvil.lock` does not prove that all files match the new templates.
An `.anvil-proposed` file contains proposed replacement content.

1. Install the selected release with `cargo install --locked --version VERSION cargo-anvil`.
2. Run `cargo anvil --version` to make sure that the selected release is active.
3. Run `cargo anvil --dry-run` and review every proposed update, refusal, and preserved customization.
4. Compare each customized file with the selected release's template. Prefer the template unless a current repository requirement needs an override.
5. If a file must return to generated content, remove only that file after reviewing its differences. Run `cargo anvil` to recreate it.
6. Run `cargo anvil` to apply the remaining updates.
7. Review the generated changes and `.anvil.lock`. Run `cargo anvil --dry-run` again and explain every remaining override.

Move repository requirements to supported Anvil configuration where possible.
Do not preserve an entire generated file to retain obsolete pins or comments.
Send general runner fixes upstream rather than maintaining permanent local copies.

External GitHub Actions require full-length commit SHA pins with release-version
comments, including actions with immutable releases.
Local action and workflow references remain local paths.
If an Anvil template still uses an external action tag, resolve that upstream.
Do not claim complete SHA pinning while generated templates still contain tags.

The current example-runner override builds each example before its execution
timeout starts. This keeps compilation outside the runtime limit.
Review this override against the selected template during each upgrade.
The PR workflow uses the unmodified Anvil template.

## Verification behavior

Coverage collection uses `cargo coverage-gate run` on the pinned nightly toolchain.
It runs both `--all-features` and `--no-default-features` configurations.
The local gate and CI uploads use the same filtered LCOV files, which contain
line-coverage counts, under `target/coverage`.
Ordinary inline `#[cfg(test)]` modules need no `coverage(off)` attribute.
If macros make source ownership uncertain, the filter keeps ambiguous counters.
Keep explicit exclusions for exceptional production paths and helpers that the
filter cannot classify.

Anvil runs examples by default. Fetch examples that send real requests use
public endpoints such as `https://example.com`. Automated runs require public
network access and can fail if the endpoint or network is unavailable.
Examples that need developer credentials, manual arguments, or separately
installed profiling tools remain build-only in
`[package.metadata.anvil.examples].no-run`.
Server examples use OS-assigned ports by default. Axum and Routerama stop
after two seconds during automated runs. Fetch streaming uses an unnamed
temporary file. Snapshot examples write their named artifacts to the current
directory.

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

Mutation testing uses Cargo Gamma through the generated Anvil recipes.
`gamma.toml` holds the shared repository configuration.
It enables all features, selects the `mutants` Cargo profile, and requires the lockfile.
Gamma selects unit and integration test targets itself.
Do not add `--tests` to `cargo-args`.
The configuration retains intentional-abort and real-time stress filters.
Type-system contracts remain in the native and careful suites.
Test helpers recognize Gamma's `CARGO_GAMMA` process marker.
They retain the legacy `MUTATION_TESTING=1` opt-in.
Mutation runs keep watchdogs and stdout/file logging disabled.

The `rallocator`, `seismograph_rallocator`, `http_headers`, and `http_headers_simd`
crates remain outside mutation-testing scope on every runner architecture.
Shared test fixtures and terminal presentation code retain their file exclusions.
Ordinary tests, coverage, and runtime-analysis selection remain unchanged.
Trait exclusions cover diagnostic formatting and redacted output.
Gamma matches the final written trait name, not a resolved trait identity.

Gamma evaluates host and feature conditions during discovery.
Inactive platform code no longer needs duplicated host configurations.
Unknown conditions remain discoverable rather than becoming silent exclusions.
The old `.cargo/mutants*.toml` files and `mutants` dependency are removed.
Gamma does not consume those files or honor `mutants::skip`.
Legacy function-wide skips and cargo-mutants description patterns are not translated into Gamma suppressions.

Do not add mutation suppressions in advance.
Run Gamma first and review its actual findings.
If a finding needs suppression, add a narrow, compiler-checked directive beside the source and explain the reason.
Do not suppress a whole function or file to silence one finding.
Gamma bounds mutant execution with time and memory limits.
A timeout is an undetected mutant, not a successful assertion.
Review hangs against the actual Gamma finding before adding a suppression.

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

## Crate designs

- [`fakeable`](fakeable.md) describes the generated real/fake wrapper model and
  its optional Mockall integration.
