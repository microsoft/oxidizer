# Metabench native contracts

The repository-specific **Metabench native contracts (Linux)** CI job installs
Valgrind and `gungraun-runner` 0.19.4, matching the workspace's Gungraun version.
It runs the native listing and default-engine smoke contracts and is required
by the stable **Required repository checks** fan-in. These are correctness
checks, not timed performance measurements.

To run the same required tier locally on Linux:

```sh
mkdir -p target/metabench-contract-scratch
export TMPDIR="$PWD/target/metabench-contract-scratch"
export CARGO_BUILD_JOBS=2 METABENCH_REQUIRE_NATIVE_CONTRACTS=1
cargo install --locked --version 0.19.4 gungraun-runner
# Install Valgrind using your Linux distribution's package manager.
gungraun-runner --version
valgrind --version
cargo test --package metabench --test spawned_benchmark --locked gungraun_listing_is_forwarded -- --exact --nocapture
cargo test --package metabench --test spawned_benchmark --locked default_engine_smoke_checks_do_not_publish_measurements -- --exact --nocapture
```

`METABENCH_REQUIRE_NATIVE_CONTRACTS` makes missing prerequisites a test failure,
not a successful skip. Without this setting, ordinary local test runs retain
the optional skip when `gungraun-runner` is unavailable. The Linux smoke contract
is not compiled on Windows; Windows' unsupported native-mode behavior is
unchanged. Test output reports each executed contract by name.

Unit regressions also check collector activation, report boundary values,
generated probe registration, bounded pipe draining, and exact worker cleanup
deadlines. Platform-specific mutation configurations exclude only adapters
that the host does not compile and explicitly documented equivalent flag
mutations; the active platform retains its behavioral checks.

Windows pipe regressions run in a subprocess with an independent deadline so
a broken nonblocking read fails promptly instead of hanging the test suite.
Reparse-point admission tests exercise file-attribute checks without requiring
symlink privileges, and private-directory creation is checked on every host.

Scripted retention and drain tests use an in-memory output tail without files
or OS pipes. Private platform tests remain beside their inaccessible internals;
the public worker contracts are exercised by `tests/spawned_benchmark.rs`.
Mutation runs skip only the real-time sustained-output timeout test. The
finite `FakeClock`/`FakeProcess` deadline regressions remain selected, while
ordinary test runs retain the actual process stress scenario.
