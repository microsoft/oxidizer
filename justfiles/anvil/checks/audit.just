# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Unscoped (consults external advisory DB; reads Cargo.lock).
[script("pwsh")]
anvil-audit: anvil-audit-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo audit
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-audit-setup installer="install": (anvil-tool-cargo-audit-install installer)

[group("anvil-setup")]
anvil-audit-validate-prereqs: anvil-tool-cargo-audit-validate-prereqs
