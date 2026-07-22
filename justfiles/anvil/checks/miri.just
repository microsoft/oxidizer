# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Nightly checks always run full-workspace; impact env vars aren't set
# by the scheduled workflow, so the affected-tier default (--workspace)
# applies. Skip guards are still included for local diff-scoped runs.

[script("pwsh")]
anvil-miri: anvil-miri-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    # Run miri via libtest (`cargo miri test`), NOT `cargo miri nextest
    # run`. nextest uses a process-per-test model; under miri every
    # process pays the full cost of re-interpreting std initialization,
    # so process-per-test roughly DOUBLES miri wall-time on a large suite
    # (the dominant cost on the PR critical path). libtest runs all tests
    # of a binary in a single process, matching how upstream runs miri.
    #
    # `--tests` selects the same target set nextest ran -- lib unit tests,
    # bin unit tests, and integration tests -- while excluding doctests
    # (miri cannot run them) and benches/examples. We use `--tests` rather
    # than `--lib --tests` because `--lib` errors with "no library targets
    # found" on a bin-only affected package under impact scoping; `--tests`
    # already covers lib unit tests and tolerates libless packages. We do
    # NOT use `--all-targets`: it would add `--benches` (criterion benches
    # have `harness = false` and run a full measurement loop -- ruinously
    # slow under miri) and `--examples` (runs each example's `main`, often
    # miri-incompatible I/O), without adding doctest coverage.
    #
    # `--all-features` exercises feature-gated code so UB hidden behind an
    # optional feature is caught, matching how upstream runs miri.
    #
    # libtest exits 0 when a binary has zero runnable tests (e.g. a crate
    # whose tests are all `#[cfg_attr(miri, ignore)]` -- the canonical
    # opt-out for build-tooling / CLI crates), so the old nextest
    # `--no-tests=pass` workaround is no longer needed.
    & cargo '+{{ rust_nightly }}' miri test --all-features --tests @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# --- pr-runtime-analysis members ---

[group("anvil-setup")]
anvil-miri-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install

[group("anvil-setup")]
anvil-miri-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs
