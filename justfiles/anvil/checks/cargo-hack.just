# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Required tier. cargo-hack's feature powerset cascades through dep
# features, so the required set (workspace-internal transitive deps)
# is the right scope.
[script("pwsh")]
anvil-cargo-hack: anvil-cargo-hack-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_REQUIRED -eq '--skip') { exit 0 }
    & cargo hack @(if ($env:ANVIL_INCLUDE_REQUIRED) { -split $env:ANVIL_INCLUDE_REQUIRED } else { '--workspace' }) --feature-powerset --depth 2 check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-cargo-hack-setup installer="install": (anvil-tool-cargo-hack-install installer)

[group("anvil-setup")]
anvil-cargo-hack-validate-prereqs: anvil-tool-cargo-hack-validate-prereqs
