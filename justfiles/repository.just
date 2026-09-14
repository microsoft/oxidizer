# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# Apply license boilerplate headers.
license: anvil-license-headers-validate-prereqs
    cargo heather --fix

# Run the Pester suite for the release-related PowerShell scripts.
[arg("scope", long, pattern='(?:unit|integration|scenarios)?')]
[script("pwsh", "-NoProfile")]
test-scripts scope="":
    $ErrorActionPreference = "Stop"
    & ./scripts/tests/Pester/Run-Tests.ps1 -Path "{{ scope }}"

# Run compile-fail tests while iterating on macro diagnostics.
[arg("package", long)]
[arg("filter", long)]
[script("pwsh", "-NoProfile")]
trybuild package filter="": anvil-tool-rustc-validate-prereqs
    $ErrorActionPreference = "Stop"
    cargo {{_anvil_stable_toolchain_args}} test --package "{{ package }}" --all-features --locked --tests -- "{{ filter }}"

# Rewrite compile-fail diagnostic snapshots after an intentional change.
[arg("package", long)]
[arg("filter", long)]
[script("pwsh", "-NoProfile")]
trybuild-overwrite package filter="": anvil-tool-rustc-validate-prereqs
    $ErrorActionPreference = "Stop"
    $env:TRYBUILD = "overwrite"
    cargo {{_anvil_stable_toolchain_args}} test --package "{{ package }}" --all-features --locked --tests -- "{{ filter }}"

# Install the Linux-only Callgrind benchmark runner.
[script("pwsh", "-NoProfile")]
setup-callgrind:
    $ErrorActionPreference = "Stop"
    & ./scripts/install-callgrind-tools.ps1

# Publish a GitHub release for a crate tag.
[arg("repository", long)]
[arg("tag", long)]
publish-gh-release repository tag: anvil-toolchain-nightly-validate-prereqs
    cargo '+{{ rust_nightly }}' -Zscript scripts/publish-gh-release.rs --repo "{{ repository }}" "{{ tag }}"
