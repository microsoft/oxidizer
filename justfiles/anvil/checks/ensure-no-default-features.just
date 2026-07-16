# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Modified tier.
[script("pwsh")]
anvil-ensure-no-default-features: anvil-ensure-no-default-features-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo ensure-no-default-features
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-ensure-no-default-features-setup installer="install": (anvil-tool-cargo-ensure-no-default-features-install installer)

[group("anvil-setup")]
anvil-ensure-no-default-features-validate-prereqs: anvil-tool-cargo-ensure-no-default-features-validate-prereqs
