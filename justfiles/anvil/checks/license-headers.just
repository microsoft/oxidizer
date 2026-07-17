# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Modified tier.
[script("pwsh")]
anvil-license-headers: anvil-license-headers-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo heather
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-license-headers-setup installer="install": (anvil-tool-cargo-heather-install installer)

[group("anvil-setup")]
anvil-license-headers-validate-prereqs: anvil-tool-cargo-heather-validate-prereqs
