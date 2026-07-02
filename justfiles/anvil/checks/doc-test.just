# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Affected tier. Two cargo-test invocations cover both feature
# configurations: --all-features catches doctests that only compile
# with optional features enabled; the default-features run catches
# doctests that only compile when those features are off. The
# duplication is cheap because doctests are already a small fraction
# of total compile/test time.
[script("pwsh")]
anvil-doc-test: anvil-doc-test-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    & cargo test --doc @pkg --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    & cargo test --doc @pkg --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-doc-test-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-doc-test-validate-prereqs: anvil-tool-rustc-validate-prereqs
