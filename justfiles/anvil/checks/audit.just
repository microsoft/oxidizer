# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Unscoped (consults external advisory DB; reads Cargo.lock).

# Audit dependencies for known security vulnerabilities.
[script("pwsh")]
anvil-audit: anvil-audit-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo audit
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-audit` recipe.
[group("anvil-setup")]
anvil-audit-setup installer="install": (anvil-tool-cargo-audit-install installer)

# Validate prerequisites for the `anvil-audit` recipe.
[group("anvil-setup")]
anvil-audit-validate-prereqs: anvil-tool-cargo-audit-validate-prereqs
