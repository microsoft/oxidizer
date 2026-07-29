# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Affected tier. Two cargo-test invocations cover both feature
# configurations: --all-features catches doctests that only compile
# with optional features enabled; the default-features run catches
# doctests that only compile when those features are off. The
# duplication is cheap because doctests are already a small fraction
# of total compile/test time.

# Run documentation tests for affected workspace packages.
[script("pwsh")]
anvil-doc-test: anvil-doc-test-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    & cargo test --doc @pkg --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    & cargo test --doc @pkg --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-doc-test` recipe.
[group("anvil-setup")]
anvil-doc-test-setup installer="install": anvil-tool-rustc-validate-prereqs

# Validate prerequisites for the `anvil-doc-test` recipe.
[group("anvil-setup")]
anvil-doc-test-validate-prereqs: anvil-tool-rustc-validate-prereqs
