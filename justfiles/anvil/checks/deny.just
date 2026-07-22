# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Unscoped (consults external advisory DB; reads Cargo.lock, not
# workspace members). Single command, but run under pwsh so it does not
# depend on the adopter's default shell (absent/varying on Windows).
[script("pwsh")]
anvil-deny: anvil-deny-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo deny check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-deny-setup installer="install": (anvil-tool-cargo-deny-install installer)

[group("anvil-setup")]
anvil-deny-validate-prereqs: anvil-tool-cargo-deny-validate-prereqs
