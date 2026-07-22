# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Race coverage rotates the miri-many-seeds window daily by
# day-of-month: day N -> seeds [2N-1, 2N+1) (2 seeds/day,
# ~62 seeds/month). Rotating amortizes the seed space across the
# schedule rather than retesting the same seeds every night; race
# conditions surface as inter-seed nondeterminism rather than
# per-seed crashes, so coverage matters more than depth-per-seed.
[script("pwsh")]
anvil-miri-race-coverage: anvil-miri-race-coverage-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $day = (Get-Date).Day
    $low = (2 * $day) - 1
    $high = (2 * $day) + 1
    $env:MIRIFLAGS = "-Zmiri-many-seeds=$low..$high $($env:MIRIFLAGS)".Trim()
    $env:RUSTFLAGS = "--cfg miri_race_coverage $($env:RUSTFLAGS)".Trim()
    Write-Host "anvil-miri-race-coverage: seed window $low..$high (day $day)"
    & cargo '+{{ rust_nightly }}' miri test --all-features --tests @pkg
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-miri-race-coverage-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install

[group("anvil-setup")]
anvil-miri-race-coverage-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs
