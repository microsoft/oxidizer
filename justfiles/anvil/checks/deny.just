# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Unscoped (consults external advisory DB; reads Cargo.lock, not
# workspace members). Single command, but run under pwsh so it does not
# depend on the adopter's default shell (absent/varying on Windows).

# Check dependency licenses, bans, sources, and advisories.
[script("pwsh")]
anvil-deny: anvil-deny-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo deny check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Install prerequisites for the `anvil-deny` recipe.
[group("anvil-setup")]
anvil-deny-setup installer="install": (anvil-tool-cargo-deny-install installer)

# Validate prerequisites for the `anvil-deny` recipe.
[group("anvil-setup")]
anvil-deny-validate-prereqs: anvil-tool-cargo-deny-validate-prereqs
