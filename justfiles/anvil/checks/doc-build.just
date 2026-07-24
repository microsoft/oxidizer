# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Required tier. cargo doc resolves intra-doc links through the dep
# graph, so a dep changing its public API can break doc-build in a
# crate that wasn't itself modified.

# Build documentation for required workspace packages.
[script("pwsh")]
anvil-doc-build: anvil-doc-build-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_REQUIRED -eq '--skip') { exit 0 }
    $env:RUSTDOCFLAGS = '-D warnings'
    & cargo doc @(if ($env:ANVIL_INCLUDE_REQUIRED) { -split $env:ANVIL_INCLUDE_REQUIRED } else { '--workspace' }) --all-features --no-deps
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# doc-build, examples and doc-test are pure cargo built-ins; the rust
# toolchain (rustc + cargo) is the only prerequisite, and we already
# rely on it being present everywhere anvil runs.

# Install prerequisites for the `anvil-doc-build` recipe.
[group("anvil-setup")]
anvil-doc-build-setup installer="install": anvil-tool-rustc-validate-prereqs

# Validate prerequisites for the `anvil-doc-build` recipe.
[group("anvil-setup")]
anvil-doc-build-validate-prereqs: anvil-tool-rustc-validate-prereqs
