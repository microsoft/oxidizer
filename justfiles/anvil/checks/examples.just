# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Affected tier.

# Build all workspace examples.
[script("pwsh")]
anvil-examples: anvil-examples-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    & cargo build @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) --examples --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-examples` recipe.
[group("anvil-setup")]
anvil-examples-setup installer="install": anvil-tool-rustc-validate-prereqs

# Validate prerequisites for the `anvil-examples` recipe.
[group("anvil-setup")]
anvil-examples-validate-prereqs: anvil-tool-rustc-validate-prereqs
