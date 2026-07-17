# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Affected tier. Per project policy, clippy runs on the affected set
# rather than the modified set: a change in a crate can introduce a
# clippy issue in a dependent crate (e.g., trait-bound or
# obviously-truthy-condition lints that key off the changed type), so
# we want downstream rev-deps to lint as well. cargo-clippy is a
# rustup component; same reasoning as fmt for the require.
[script("pwsh")]
anvil-clippy: anvil-clippy-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    & cargo clippy @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) --all-targets --all-features --locked "--" '-D' 'warnings'
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-clippy-setup installer="install": anvil-component-default-clippy-install

[group("anvil-setup")]
anvil-clippy-validate-prereqs: anvil-component-default-clippy-validate-prereqs
