# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Affected tier.
[script("pwsh")]
anvil-bench: anvil-bench-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    & cargo bench @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) --all-features --no-run
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# bench uses cargo-built-ins (cargo bench --no-run + plain bench runs);
# no extra tool install needed beyond the rust toolchain.
[group("anvil-setup")]
anvil-bench-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-bench-validate-prereqs: anvil-tool-rustc-validate-prereqs
