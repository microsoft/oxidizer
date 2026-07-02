# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Modified tier.
[script("pwsh")]
anvil-ensure-no-cyclic-deps: anvil-ensure-no-cyclic-deps-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo ensure-no-cyclic-deps
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-ensure-no-cyclic-deps-setup installer="install": (anvil-tool-cargo-ensure-no-cyclic-deps-install installer)

[group("anvil-setup")]
anvil-ensure-no-cyclic-deps-validate-prereqs: anvil-tool-cargo-ensure-no-cyclic-deps-validate-prereqs
