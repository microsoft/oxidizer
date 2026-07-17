# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Required tier. cargo doc resolves intra-doc links through the dep
# graph, so a dep changing its public API can break doc-build in a
# crate that wasn't itself modified.
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
[group("anvil-setup")]
anvil-doc-build-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-doc-build-validate-prereqs: anvil-tool-rustc-validate-prereqs
