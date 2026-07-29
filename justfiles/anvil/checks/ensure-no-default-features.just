# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Modified tier.

# Check that workspace packages compile without default features.
[script("pwsh")]
anvil-ensure-no-default-features: anvil-ensure-no-default-features-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo ensure-no-default-features
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-ensure-no-default-features` recipe.
[group("anvil-setup")]
anvil-ensure-no-default-features-setup installer="install": (anvil-tool-cargo-ensure-no-default-features-install installer)

# Validate prerequisites for the `anvil-ensure-no-default-features` recipe.
[group("anvil-setup")]
anvil-ensure-no-default-features-validate-prereqs: anvil-tool-cargo-ensure-no-default-features-validate-prereqs
