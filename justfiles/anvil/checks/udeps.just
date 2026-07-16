# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Required tier. cargo-udeps detects unused dependencies by resolving
# the full crate graph and seeing which deps are referenced; that's
# precisely what the required tier is for. Pinned to the general
# nightly defined in versions.just.
#
# Run twice, because cargo-udeps only analyzes the targets it is told
# to, and each invocation catches a variant the other masks:
#   1. default targets (lib + bins): catches a dep in `[dependencies]`
#      that is referenced only by tests/benches/examples -- it belongs
#      in `[dev-dependencies]`. Under `--all-targets` such a dep looks
#      "used" (the test/bench/example target satisfies the lookup), so
#      this run is the only one that surfaces it.
#   2. `--all-targets`: catches unused `[dev-dependencies]` -- which the
#      default-targets run never compiles, so it is the only one that
#      surfaces those.
# A genuinely-unused dep is caught by both; the two together cover all
# of {unused dep, unused dev-dep, dep that should be a dev-dep}.
[script("pwsh")]
anvil-udeps: anvil-udeps-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_REQUIRED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_REQUIRED) { -split $env:ANVIL_INCLUDE_REQUIRED } else { '--workspace' })
    # Pass 1: default targets (lib + bins) -- surfaces [dependencies] that
    # are only used by tests/benches/examples (should be dev-deps).
    & cargo '+{{ rust_nightly }}' udeps @pkg --all-features
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    # Pass 2: --all-targets -- surfaces unused [dev-dependencies].
    & cargo '+{{ rust_nightly }}' udeps @pkg --all-features --all-targets
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-udeps-setup installer="install": anvil-toolchain-nightly-install (anvil-tool-cargo-udeps-install installer)

[group("anvil-setup")]
anvil-udeps-validate-prereqs: anvil-toolchain-nightly-validate-prereqs anvil-tool-cargo-udeps-validate-prereqs
