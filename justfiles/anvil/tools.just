# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../docs/design/local.md for the policy.

# ============================================================================
# System-level prerequisites (libclang, etc.)
# ============================================================================

# Public: probe for system-level prerequisites that catalog tools need
# to BUILD from source. The `binstall` install path downloads pre-built
# binaries and does not need these, so this check is primarily relevant
# for the source-build `install` path (used by local devs and the ADO
# backend) and is best-effort (skipped silently) for `binstall`.
#
# Scope policy: only system libs that an anvil catalog tool DIRECTLY
# requires. We do not try to be a general-purpose dev-env doctor.
# Current entries:
#   - libclang: required by cargo-spellcheck (clang-sys / hunspell-rs)
#     at build time.
#
# Detection uses presence-only probes (file existence in standard install
# dirs + `LIBCLANG_PATH` env var). No version checks -- system libs upgrade
# independently and any reasonably modern libclang works for clang-sys.
#
# On missing deps the recipe prints copy-paste install hints per OS /
# package manager and exits non-zero. No auto-install: admin / sudo and
# package-manager choice stay with the user.
[group("anvil-setup")]
[script("pwsh")]
anvil-system-deps-check:
    $ErrorActionPreference = 'Stop'
    $missing = @()

    # libclang: required to BUILD cargo-spellcheck from source.
    $haveLibclang = $false
    $libclangFiles = @('libclang.dll', 'libclang.so', 'libclang.so.1', 'libclang.dylib')
    if ($env:LIBCLANG_PATH) {
        foreach ($f in $libclangFiles) {
            if (Test-Path (Join-Path $env:LIBCLANG_PATH $f)) { $haveLibclang = $true; break }
        }
    }
    if (-not $haveLibclang) {
        $probes = if ($IsWindows) {
            @(
                'C:\Program Files\LLVM\bin\libclang.dll',
                "$env:USERPROFILE\scoop\apps\llvm\current\bin\libclang.dll"
            )
        } elseif ($IsMacOS) {
            @(
                '/usr/local/opt/llvm/lib/libclang.dylib',
                '/opt/homebrew/opt/llvm/lib/libclang.dylib'
            )
        } else {
            @(
                '/usr/lib/x86_64-linux-gnu/libclang.so.1',
                '/usr/lib/aarch64-linux-gnu/libclang.so.1',
                '/usr/lib64/libclang.so',
                '/usr/lib64/libclang.so.1'
            )
        }
        foreach ($p in $probes) {
            if (Get-Item -LiteralPath $p -ErrorAction SilentlyContinue) { $haveLibclang = $true; break }
        }
        # Linux distros often add a version suffix (libclang-19.so etc.); glob fallback.
        if (-not $haveLibclang -and -not $IsWindows -and -not $IsMacOS) {
            $glob = Get-ChildItem -Path '/usr/lib','/usr/lib64','/usr/lib/x86_64-linux-gnu','/usr/lib/aarch64-linux-gnu' -Filter 'libclang*.so*' -ErrorAction SilentlyContinue
            if ($glob) { $haveLibclang = $true }
        }
    }
    if (-not $haveLibclang) {
        $missing += [pscustomobject]@{
            name = 'libclang'
            why = 'cargo-spellcheck (clang-sys/hunspell-rs) needs libclang at build time'
            hints = @(
                'Linux (Ubuntu/Debian):    sudo apt-get install -y libclang-dev'
                'Linux (Azure Linux/RHEL): sudo tdnf install -y clang-devel'
                'macOS (homebrew):         brew install llvm'
                'Windows (scoop):          scoop install llvm     # no admin'
                'Windows (winget, admin):  winget install LLVM.LLVM'
            )
        }
    }

    if ($missing.Count -gt 0) {
        Write-Host ''
        foreach ($m in $missing) {
            Write-Host "anvil: missing system dependency '$($m.name)'" -ForegroundColor Yellow
            Write-Host "  why: $($m.why)"
            Write-Host '  install:'
            foreach ($h in $m.hints) { Write-Host "    $h" }
            Write-Host ''
        }
        Write-Host "If the dep is installed but not on the standard search path, set LIBCLANG_PATH and re-run." -ForegroundColor Yellow
        exit 1
    }

# ============================================================================
# rustc + pwsh validate-prereqs (no install -- system prereqs)
# ============================================================================
#
# rustc is installed via rustup (https://rustup.rs); pwsh is installed
# via the platform's package manager. Both are presumed present on any
# machine running anvil; the validate recipes just surface a friendly
# error if not.

[group("anvil-setup")]
[script("pwsh")]
anvil-tool-rustc-validate-prereqs:
    if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        Write-Error 'anvil: rustc not found. Install via rustup: https://rustup.rs'
        exit 1
    }

[group("anvil-setup")]
[script("pwsh")]
anvil-tool-pwsh-validate-prereqs:
    if (-not (Get-Command pwsh -ErrorAction SilentlyContinue)) {
        $hint = if ($IsMacOS) {
            'brew install --cask powershell'
        } elseif ($IsLinux) {
            'see https://github.com/PowerShell/PowerShell'
        } else {
            'winget install --id Microsoft.PowerShell'
        }
        Write-Error "anvil: pwsh (PowerShell Core) not found. Install: $hint"
        exit 1
    }

# ============================================================================
# Private helpers (install/check primitives)
# ============================================================================

# _install-tool: install a cargo subcommand at exactly the pinned version,
# or no-op if it is already installed at or above that version. The
# `installer` parameter selects between:
#   - "install"  (cargo install --locked, pure-source). Default.
#   - "binstall" (cargo binstall --no-confirm --locked, with cargo install
#                 fallback if binstall fails). Bootstraps cargo-binstall
#                 itself if not on PATH.
[script("pwsh")]
_install-tool name version installer:
    $ErrorActionPreference = 'Stop'
    $name = '{{name}}'
    $version = '{{version}}'
    $installer = '{{installer}}'

    if ($installer -ne 'install' -and $installer -ne 'binstall') {
        Write-Error "_install-tool: unknown installer '$installer' (expected 'install' or 'binstall')"
        exit 2
    }

    # Already at or above the pin: skip. We don't downgrade tools the
    # user upgraded for their own reasons; the validate side uses
    # `installed >= pin`, so newer is fine. The early-exit is also what
    # makes the actions/cache restore actually useful -- without it,
    # every post-restore run would try to re-install on top of the
    # cached binaries and fail with "binary already exists in destination".
    $installed = $null
    $pattern = '^' + [regex]::Escape($name) + ' v(\S+):'
    foreach ($line in (cargo install --list 2>$null)) {
        if ($line -match $pattern) { $installed = $Matches[1]; break }
    }
    if ($installed) {
        try {
            if (([version]$installed) -ge ([version]$version)) {
                Write-Host "$name >= $version (already satisfied; installed=$installed)"
                exit 0
            }
        } catch {
            # Fall through to reinstall when versions don't parse as [version].
        }
    }

    Write-Host "Installing $name =$version (installer: $installer)"
    if ($installer -eq 'binstall') {
        if (-not (Get-Command cargo-binstall -ErrorAction SilentlyContinue)) {
            Write-Host '  Bootstrapping cargo-binstall'
            cargo install --locked cargo-binstall
            if ($LASTEXITCODE -ne 0) {
                Write-Error 'cargo-binstall bootstrap failed'
                exit $LASTEXITCODE
            }
        }
        cargo binstall --no-confirm --locked $name --version "=$version"
        if ($LASTEXITCODE -eq 0) { exit 0 }
        Write-Host '  binstall failed; falling back to cargo install' -ForegroundColor Yellow
    }
    cargo install --locked $name --version "=$version"
    if ($LASTEXITCODE -ne 0) {
        Write-Error "$name install FAILED"
        exit $LASTEXITCODE
    }

# _check-tool: verify a cargo subcommand is installed at or above the
# pinned version. Errors with an install hint on missing or too-old.
[script("pwsh")]
_check-tool name version:
    $ErrorActionPreference = 'Stop'
    $name = '{{name}}'
    $version = '{{version}}'

    $installed = $null
    $pattern = '^' + [regex]::Escape($name) + ' v(\S+):'
    foreach ($line in (cargo install --list 2>$null)) {
        if ($line -match $pattern) { $installed = $Matches[1]; break }
    }
    if (-not $installed) {
        Write-Error "anvil: required tool '$name' not found. Install: cargo install --locked --version =$version $name"
        exit 1
    }
    try {
        $installedV = [version]$installed
        $pinV = [version]$version
    } catch {
        Write-Error "anvil: cannot compare versions for '$name' (installed=$installed pin=$version). Reinstall: cargo install --locked --version =$version $name"
        exit 1
    }
    if ($installedV -lt $pinV) {
        Write-Error "anvil: '$name' v$installed is older than the required minimum v$version. Upgrade: cargo install --locked --version =$version $name"
        exit 1
    }

# _install-toolchain: install a specific rustup toolchain (no components).
[script("pwsh")]
_install-toolchain toolchain:
    $ErrorActionPreference = 'Stop'
    $toolchain = '{{toolchain}}'
    Write-Host "rustup toolchain install $toolchain --profile minimal --no-self-update"
    rustup toolchain install $toolchain --profile minimal --no-self-update
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Failed to install toolchain '$toolchain'"
        exit $LASTEXITCODE
    }

# _check-toolchain: verify a rustup toolchain is installed.
[script("pwsh")]
_check-toolchain toolchain:
    $ErrorActionPreference = 'Stop'
    $toolchain = '{{toolchain}}'
    rustup which --toolchain $toolchain rustc 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Error "anvil: toolchain '$toolchain' not installed. Run: rustup toolchain install $toolchain --profile minimal"
        exit 1
    }

# _install-component: add a component to a toolchain. The toolchain is
# either the literal "default" (current rustup default) or a specific
# pinned toolchain string (which must already be installed -- the
# per-component setup recipes ensure this via a dependency on
# anvil-<toolchain>-install).
[script("pwsh")]
_install-component toolchain component:
    $ErrorActionPreference = 'Stop'
    $toolchain = '{{toolchain}}'
    $component = '{{component}}'
    if ($toolchain -eq 'default') {
        Write-Host "rustup component add $component (default toolchain)"
        rustup component add $component
    } else {
        Write-Host "rustup component add --toolchain $toolchain $component"
        rustup component add --toolchain $toolchain $component
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Failed to install component '$component' on toolchain '$toolchain'"
        exit $LASTEXITCODE
    }

# _check-component: verify a component is installed on a toolchain.
[script("pwsh")]
_check-component toolchain component:
    $ErrorActionPreference = 'Stop'
    $toolchain = '{{toolchain}}'
    $component = '{{component}}'
    $output = if ($toolchain -eq 'default') {
        rustup component list --installed 2>$null
    } else {
        rustup component list --installed --toolchain $toolchain 2>$null
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Error "anvil: toolchain '$toolchain' not installed. Run: rustup toolchain install $toolchain --profile minimal"
        exit 1
    }
    $found = $output | Where-Object { $_ -like "$component*" }
    if (-not $found) {
        $cmd = if ($toolchain -eq 'default') { "rustup component add $component" } else { "rustup component add --toolchain $toolchain $component" }
        Write-Error "anvil: component '$component' not installed on toolchain '$toolchain'. Run: $cmd"
        exit 1
    }

# ============================================================================
# Rustup toolchains
# ============================================================================

[group("anvil-setup")]
anvil-toolchain-nightly-install: (_install-toolchain rust_nightly)

[group("anvil-setup")]
anvil-toolchain-nightly-validate-prereqs: (_check-toolchain rust_nightly)

[group("anvil-setup")]
anvil-toolchain-nightly-external-types-install: (_install-toolchain rust_nightly_external_types)

[group("anvil-setup")]
anvil-toolchain-nightly-external-types-validate-prereqs: (_check-toolchain rust_nightly_external_types)

# ============================================================================
# Rustup components
# ============================================================================
#
# Default-toolchain components are installed via `rustup component add`
# (no toolchain spec). Nightly components depend on the toolchain
# being installed first (via the relevant anvil-<toolchain>-install
# recipe) and then add the component.

[group("anvil-setup")]
anvil-component-default-clippy-install: (_install-component "default" "clippy")

[group("anvil-setup")]
anvil-component-default-clippy-validate-prereqs: (_check-component "default" "clippy")

[group("anvil-setup")]
anvil-component-default-rustfmt-install: (_install-component "default" "rustfmt")

[group("anvil-setup")]
anvil-component-default-rustfmt-validate-prereqs: (_check-component "default" "rustfmt")

[group("anvil-setup")]
anvil-component-nightly-rustfmt-install: anvil-toolchain-nightly-install (_install-component rust_nightly "rustfmt")

[group("anvil-setup")]
anvil-component-nightly-rustfmt-validate-prereqs: anvil-toolchain-nightly-validate-prereqs (_check-component rust_nightly "rustfmt")

[group("anvil-setup")]
anvil-component-nightly-miri-install: anvil-toolchain-nightly-install (_install-component rust_nightly "miri")

[group("anvil-setup")]
anvil-component-nightly-miri-validate-prereqs: anvil-toolchain-nightly-validate-prereqs (_check-component rust_nightly "miri")

[group("anvil-setup")]
anvil-component-nightly-rust-src-install: anvil-toolchain-nightly-install (_install-component rust_nightly "rust-src")

[group("anvil-setup")]
anvil-component-nightly-rust-src-validate-prereqs: anvil-toolchain-nightly-validate-prereqs (_check-component rust_nightly "rust-src")

# llvm-tools ships the llvm-profdata / llvm-cov binaries that
# cargo-llvm-cov shells out to. Required on the nightly toolchain
# because anvil runs coverage on nightly (so cargo-llvm-cov sets the
# `cfg(coverage_nightly)` that the workspace's
# `#[cfg_attr(coverage_nightly, coverage(off))]` exclusions depend on).
# Use the canonical `llvm-tools` name (not the legacy `llvm-tools-preview`
# alias): `rustup component list` reports it as `llvm-tools-<target>`, so
# the `-preview` alias never prefix-matches in `_check-component`.
[group("anvil-setup")]
anvil-component-nightly-llvm-tools-install: anvil-toolchain-nightly-install (_install-component rust_nightly "llvm-tools")

[group("anvil-setup")]
anvil-component-nightly-llvm-tools-validate-prereqs: anvil-toolchain-nightly-validate-prereqs (_check-component rust_nightly "llvm-tools")

# ============================================================================
# Cargo subcommands
# ============================================================================
#
# One pair (install + validate-prereqs) per tool, alphabetical.
# Version pins live in versions.just (one cargo_<name>_version variable
# per tool). The install recipes accept an `installer="install"|"binstall"`
# parameter; the validate-prereqs recipes do not (they only read state).

[group("anvil-setup")]
anvil-tool-cargo-aprz-install installer="install": (_install-tool "cargo-aprz" cargo_aprz_version installer)

[group("anvil-setup")]
anvil-tool-cargo-aprz-validate-prereqs: (_check-tool "cargo-aprz" cargo_aprz_version)

[group("anvil-setup")]
anvil-tool-cargo-audit-install installer="install": (_install-tool "cargo-audit" cargo_audit_version installer)

[group("anvil-setup")]
anvil-tool-cargo-audit-validate-prereqs: (_check-tool "cargo-audit" cargo_audit_version)

# cargo-bolero is Linux-only: its `bolero-afl` build dependency
# compiles AFL's native C (afl-fuzz.c), which needs POSIX headers
# (`unistd.h`) and uses preprocessor constructs MSVC rejects, so the
# install fails outright on windows-*-msvc. bolero's libfuzzer engine
# is likewise Linux-only. oxidizer's reference workflow runs bolero
# exclusively on ubuntu, so anvil follows suit: the install/validate
# pair self-skips on non-Linux (analogous to cargo-mutants skipping
# aarch64-pc-windows-msvc), and the anvil-bolero check self-skips on
# the same hosts. The pr-runtime-analysis Windows/ARM legs therefore
# treat bolero as a no-op rather than a setup failure.
[group("anvil-setup")]
[script("pwsh")]
anvil-tool-cargo-bolero-install installer="install":
    if (-not $IsLinux) {
        Write-Host 'anvil-tool-cargo-bolero-install: non-Linux host -- skipping (cargo-bolero/libfuzzer is Linux-only)'
        exit 0
    }
    & just _install-tool cargo-bolero {{cargo_bolero_version}} {{installer}}
    exit $LASTEXITCODE

[group("anvil-setup")]
[script("pwsh")]
anvil-tool-cargo-bolero-validate-prereqs:
    if (-not $IsLinux) {
        Write-Host 'anvil-tool-cargo-bolero-validate-prereqs: non-Linux host -- skipping (cargo-bolero/libfuzzer is Linux-only)'
        exit 0
    }
    & just _check-tool cargo-bolero {{cargo_bolero_version}}
    exit $LASTEXITCODE

[group("anvil-setup")]
anvil-tool-cargo-careful-install installer="install": (_install-tool "cargo-careful" cargo_careful_version installer)

[group("anvil-setup")]
anvil-tool-cargo-careful-validate-prereqs: (_check-tool "cargo-careful" cargo_careful_version)

[group("anvil-setup")]
anvil-tool-cargo-check-external-types-install installer="install": (_install-tool "cargo-check-external-types" cargo_check_external_types_version installer)

[group("anvil-setup")]
anvil-tool-cargo-check-external-types-validate-prereqs: (_check-tool "cargo-check-external-types" cargo_check_external_types_version)

[group("anvil-setup")]
anvil-tool-cargo-coverage-gate-install installer="install": (_install-tool "cargo-coverage-gate" cargo_coverage_gate_version installer)

[group("anvil-setup")]
anvil-tool-cargo-coverage-gate-validate-prereqs: (_check-tool "cargo-coverage-gate" cargo_coverage_gate_version)

[group("anvil-setup")]
anvil-tool-cargo-delta-install installer="install": (_install-tool "cargo-delta" cargo_delta_version installer)

[group("anvil-setup")]
anvil-tool-cargo-delta-validate-prereqs: (_check-tool "cargo-delta" cargo_delta_version)

[group("anvil-setup")]
anvil-tool-cargo-deny-install installer="install": (_install-tool "cargo-deny" cargo_deny_version installer)

[group("anvil-setup")]
anvil-tool-cargo-deny-validate-prereqs: (_check-tool "cargo-deny" cargo_deny_version)

[group("anvil-setup")]
anvil-tool-cargo-doc2readme-install installer="install": (_install-tool "cargo-doc2readme" cargo_doc2readme_version installer)

[group("anvil-setup")]
anvil-tool-cargo-doc2readme-validate-prereqs: (_check-tool "cargo-doc2readme" cargo_doc2readme_version)

[group("anvil-setup")]
anvil-tool-cargo-ensure-no-cyclic-deps-install installer="install": (_install-tool "cargo-ensure-no-cyclic-deps" cargo_ensure_no_cyclic_deps_version installer)

[group("anvil-setup")]
anvil-tool-cargo-ensure-no-cyclic-deps-validate-prereqs: (_check-tool "cargo-ensure-no-cyclic-deps" cargo_ensure_no_cyclic_deps_version)

[group("anvil-setup")]
anvil-tool-cargo-ensure-no-default-features-install installer="install": (_install-tool "cargo-ensure-no-default-features" cargo_ensure_no_default_features_version installer)

[group("anvil-setup")]
anvil-tool-cargo-ensure-no-default-features-validate-prereqs: (_check-tool "cargo-ensure-no-default-features" cargo_ensure_no_default_features_version)

[group("anvil-setup")]
anvil-tool-cargo-hack-install installer="install": (_install-tool "cargo-hack" cargo_hack_version installer)

[group("anvil-setup")]
anvil-tool-cargo-hack-validate-prereqs: (_check-tool "cargo-hack" cargo_hack_version)

[group("anvil-setup")]
anvil-tool-cargo-heather-install installer="install": (_install-tool "cargo-heather" cargo_heather_version installer)

[group("anvil-setup")]
anvil-tool-cargo-heather-validate-prereqs: (_check-tool "cargo-heather" cargo_heather_version)

[group("anvil-setup")]
anvil-tool-cargo-llvm-cov-install installer="install": (_install-tool "cargo-llvm-cov" cargo_llvm_cov_version installer)

[group("anvil-setup")]
anvil-tool-cargo-llvm-cov-validate-prereqs: (_check-tool "cargo-llvm-cov" cargo_llvm_cov_version)

# cargo-mutants doesn't build on aarch64-pc-windows-msvc (upstream
# winapi crate incompat). The install recipe self-skips on that target
# so per-group setup recipes that depend on it (pr-mutants-setup,
# scheduled-exhaustive-setup) don't fail on that platform. The
# mutants-diff / mutants-full check recipes also self-skip on the same
# target, so the check is effectively a no-op end-to-end there.
[group("anvil-setup")]
[script("pwsh")]
anvil-tool-cargo-mutants-install installer="install":
    if ($IsWindows -and ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64')) {
        Write-Host 'anvil-tool-cargo-mutants-install: aarch64-pc-windows-msvc -- skipping (winapi build incompat)'
        exit 0
    }
    & just _install-tool cargo-mutants {{cargo_mutants_version}} {{installer}}
    exit $LASTEXITCODE

[group("anvil-setup")]
[script("pwsh")]
anvil-tool-cargo-mutants-validate-prereqs:
    if ($IsWindows -and ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64')) {
        Write-Host 'anvil-tool-cargo-mutants-validate-prereqs: aarch64-pc-windows-msvc -- skipping (winapi build incompat)'
        exit 0
    }
    & just _check-tool cargo-mutants {{cargo_mutants_version}}
    exit $LASTEXITCODE

[group("anvil-setup")]
anvil-tool-cargo-nextest-install installer="install": (_install-tool "cargo-nextest" cargo_nextest_version installer)

[group("anvil-setup")]
anvil-tool-cargo-nextest-validate-prereqs: (_check-tool "cargo-nextest" cargo_nextest_version)

[group("anvil-setup")]
anvil-tool-cargo-semver-checks-install installer="install": (_install-tool "cargo-semver-checks" cargo_semver_checks_version installer)

[group("anvil-setup")]
anvil-tool-cargo-semver-checks-validate-prereqs: (_check-tool "cargo-semver-checks" cargo_semver_checks_version)

[group("anvil-setup")]
anvil-tool-cargo-sort-install installer="install": (_install-tool "cargo-sort" cargo_sort_version installer)

[group("anvil-setup")]
anvil-tool-cargo-sort-validate-prereqs: (_check-tool "cargo-sort" cargo_sort_version)

[group("anvil-setup")]
anvil-tool-cargo-spellcheck-install installer="install": (_install-tool "cargo-spellcheck" cargo_spellcheck_version installer)

[group("anvil-setup")]
anvil-tool-cargo-spellcheck-validate-prereqs: (_check-tool "cargo-spellcheck" cargo_spellcheck_version)

[group("anvil-setup")]
anvil-tool-cargo-udeps-install installer="install": (_install-tool "cargo-udeps" cargo_udeps_version installer)

[group("anvil-setup")]
anvil-tool-cargo-udeps-validate-prereqs: (_check-tool "cargo-udeps" cargo_udeps_version)
