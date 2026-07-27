# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Run Miri tests with strict provenance checks.
[script("pwsh")]
anvil-miri-strict-provenance: anvil-miri-strict-provenance-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $env:MIRIFLAGS = "-Zmiri-strict-provenance $($env:MIRIFLAGS)".Trim()
    $env:RUSTFLAGS = "--cfg miri_strict_provenance $($env:RUSTFLAGS)".Trim()
    & cargo '+{{ rust_nightly }}' miri test --all-features --tests @pkg
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-miri-strict-provenance` recipe.
[group("anvil-setup")]
anvil-miri-strict-provenance-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install

# Validate prerequisites for the `anvil-miri-strict-provenance` recipe.
[group("anvil-setup")]
anvil-miri-strict-provenance-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs
