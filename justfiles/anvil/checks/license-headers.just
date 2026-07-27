# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Modified tier.

# Check source files for required license headers.
[script("pwsh")]
anvil-license-headers: anvil-license-headers-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo heather
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-license-headers` recipe.
[group("anvil-setup")]
anvil-license-headers-setup installer="install": (anvil-tool-cargo-heather-install installer)

# Validate prerequisites for the `anvil-license-headers` recipe.
[group("anvil-setup")]
anvil-license-headers-validate-prereqs: anvil-tool-cargo-heather-validate-prereqs
