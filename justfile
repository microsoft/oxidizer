# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set shell := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set script-interpreter := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive"]

_default:
    @just --list

# >>> anvil-managed: anvil-imports
import 'justfiles/anvil/mod.just'
# <<< anvil-managed: anvil-imports

# Repository-specific utilities outside Cargo Anvil.

# Apply license boilerplate headers.
license: anvil-license-headers-validate-prereqs
    cargo heather --fix

# Run the Pester suite for the release-related PowerShell scripts.
[arg("scope", long, pattern='(?:unit|integration|scenarios)?')]
[script("pwsh", "-NoProfile")]
test-scripts scope="":
    $ErrorActionPreference = "Stop"
    & ./scripts/tests/Pester/Run-Tests.ps1 -Path "{{ scope }}"

# Verify the package-isolated no_std contract on native AArch64 Linux.
test-http-headers-simd-no-std-arm: (_test-http-headers-simd-no-std "aarch64-unknown-linux-gnu")

# Install the selected Rust target for baseline x86-family no_std tests.
[arg("target", long, pattern='(?:i586|i686|x86_64)-unknown-linux-gnu')]
[script("pwsh", "-NoProfile")]
setup-http-headers-simd-no-std-x86 target: anvil-toolchain-stable-install
    $ErrorActionPreference = 'Stop'
    $toolchainArgs = {{_anvil_stable_toolchain_args}}
    if ($toolchainArgs.Count -gt 0) {
        $env:RUSTUP_TOOLCHAIN = $toolchainArgs[0].Substring(1)
    }
    & rustup target add "{{ target }}"
    exit $LASTEXITCODE

# Verify baseline 64-bit or 32-bit x86 no_std dispatch on an x86-64 Linux host.
[arg("target", long, pattern='(?:i586|i686|x86_64)-unknown-linux-gnu')]
test-http-headers-simd-no-std-x86 target: (_test-http-headers-simd-no-std target)

[private]
[arg("target", pattern='(?:aarch64|i586|i686|x86_64)-unknown-linux-gnu')]
[script("pwsh", "-NoProfile")]
_test-http-headers-simd-no-std target: anvil-tool-rustc-validate-prereqs
    $ErrorActionPreference = 'Stop'
    $target = '{{ target }}'
    $hostArchitecture = if ($target -eq 'aarch64-unknown-linux-gnu') { 'Arm64' } else { 'X64' }
    if (-not $IsLinux -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne $hostArchitecture) {
        throw "isolated no_std tests for $target require a native $hostArchitecture Linux host"
    }
    if ($target -eq 'aarch64-unknown-linux-gnu') {
        $requiredTests = @(
            'dispatch::tests::no_std_neon_detection_matches_target_features: test',
            'arm::tests::neon_matches_scalar: test'
        )
    } else {
        $cpu = switch ($target) {
            'x86_64-unknown-linux-gnu' { 'x86-64' }
            'i686-unknown-linux-gnu' { 'pentium4' }
            'i586-unknown-linux-gnu' { 'pentium' }
        }
        # Encoded flags take precedence over inherited RUSTFLAGS and configured x86-64-v3.
        $env:CARGO_ENCODED_RUSTFLAGS = @("-Ctarget-cpu=$cpu", '-Ctarget-feature=-ssse3,-sse4.2') -join [char]31
        $requiredTests = @(
            'dispatch::tests::x86_feature_detection_matches_compile_time_or_runtime_support: test',
            'dispatch::tests::no_std_ssse3_detection_is_stable_after_caching: test',
            'dispatch::tests::no_std_sse42_detection_is_stable_after_caching: test'
        )
        if ($target -ne 'x86_64-unknown-linux-gnu') {
            $requiredTests += 'dispatch::tests::no_std_sse2_detection_matches_target_features: test'
        }
    }
    # Selecting the facade alongside this package would re-enable std.
    $testArgs = @('--locked', '--package', 'http_headers_simd', '--no-default-features', '--target', $target, '--tests')
    $tests = & cargo {{_anvil_stable_toolchain_args}} test @testArgs -- --list
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $tests | Write-Output
    # Fail if feature unification or target selection hides either path.
    foreach ($required in $requiredTests) {
        if ($tests -notcontains $required) {
            throw "required no_std test for $target is missing: $required"
        }
    }
    & cargo {{_anvil_stable_toolchain_args}} test @testArgs
    exit $LASTEXITCODE

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
