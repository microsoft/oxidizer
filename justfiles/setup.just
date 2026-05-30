# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# Installs the complete set of Rust toolchains and other tools for local PC development.
[script]
install-tools:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # This "full install" logic is specific to local PC setup. The GitHub workflows reuse the same
    # version numbers but have their own logic for what tooling needs to be installed in which jobs.

    # First, we install/upgrade the stable toolchain in `rust-toolchain.toml`.
    rustup toolchain install

    # Then we install the other toolchains used by specific commands.
    rustup toolchain install $env:RUST_MSRV
    rustup toolchain install $env:RUST_NIGHTLY --component miri --component rustfmt --component rust-src --component llvm-tools-preview
    rustup toolchain install $env:RUST_NIGHTLY_EXTERNAL_TYPES

    cargo install `
        cargo-careful@$env:CARGO_CAREFUL_VERSION `
        cargo-check-external-types@$env:CARGO_CHECK_EXTERNAL_TYPES_VERSION `
        cargo-deny@$env:CARGO_DENY_VERSION `
        cargo-doc2readme@$env:CARGO_DOC2README_VERSION `
        cargo-ensure-no-cyclic-deps@$env:CARGO_ENSURE_NO_CYCLIC_DEPS_VERSION `
        cargo-ensure-no-default-features@$env:CARGO_ENSURE_NO_DEFAULT_FEATURES_VERSION `
        cargo-hack@$env:CARGO_HACK_VERSION `
        cargo-heather@$env:CARGO_HEATHER_VERSION `
        cargo-llvm-cov@$env:CARGO_LLVM_COV_VERSION `
        cargo-mutants@$env:CARGO_MUTANTS_VERSION `
        cargo-nextest@$env:CARGO_NEXTEST_VERSION `
        cargo-semver-checks@$env:CARGO_SEMVER_CHECKS_VERSION `
        cargo-sort@$env:CARGO_SORT_VERSION `
        cargo-udeps@$env:CARGO_UDEPS_VERSION `
        cargo-workspaces@$env:CARGO_WORKSPACES_VERSION `
        --locked

    # This tool is not well maintained and fails to actually build if using locked dependencies. Okay then.
    cargo install cargo-spellcheck@$env:CARGO_SPELLCHECK_VERSION

    # Pester powers the release-script test suite (just test-scripts). Skip the
    # install if the user already has a compatible version, otherwise install the
    # current-user copy.
    $pester = Get-Module Pester -ListAvailable | Sort-Object Version -Descending | Select-Object -First 1
    if ($null -eq $pester -or $pester.Version -lt [version]'5.7.0') {
        Install-Module -Name Pester -MinimumVersion 5.7.1 -Force -Scope CurrentUser -SkipPublisherCheck
    } else {
        Write-Host "Pester $($pester.Version) already installed; skipping."
    }

    # Install platform-specific benchmark toolchain (Callgrind, Linux-only).
    & (Join-Path $PSScriptRoot ".." "scripts" "install-callgrind-tools.ps1")
