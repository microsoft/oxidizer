# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Affected tier. Smoke-only fuzz pass via [bolero](https://crates.io/crates/bolero):
# each fuzz target gets a 60-second libfuzzer run, just enough to
# surface obvious crashes/hangs introduced by a PR without paying for
# a long fuzzing campaign.
#
# Linux-only: cargo-bolero's libfuzzer engine (and its bolero-afl
# build dependency) only builds on Linux, so the recipe self-skips on
# non-Linux hosts -- the pr-runtime-analysis Windows/ARM legs treat
# bolero as a no-op rather than a failure. This matches oxidizer's
# reference workflow, whose bolero job runs exclusively on ubuntu.
# Crates that ship bolero harnesses still get them compiled and
# exercised as ordinary tests by anvil-llvm-cov on every leg; the
# libfuzzer campaign is the Linux-only extra.
#
# Target discovery is `cargo bolero list` over the affected packages.
# Crates with no bolero harness print zero targets and the recipe
# no-ops. The libfuzzer engine requires nightly, so the test
# invocation uses the pinned rust_nightly. Adopters who want longer
# campaigns wire a separate scheduled job; anvil deliberately does
# not ship a long-form fuzz tier (the failure surface is too
# repo-specific to standardize).

# Run Bolero tests for affected workspace packages.
[script("pwsh")]
anvil-bolero: anvil-bolero-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if (-not $IsLinux) {
        Write-Host 'anvil-bolero: non-Linux host -- skipping (cargo-bolero/libfuzzer is Linux-only; harnesses still run under anvil-llvm-cov)'
        exit 0
    }
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-bolero: no affected packages; skipping'
        exit 0
    }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Impact emits version-qualified specs (name@version); strip the
    # @version qualifier -- cargo-bolero resolves over workspace members
    # only, so bare names suffice and avoid any spec-parsing surprises.
    $bareArgs = [System.Collections.Generic.List[string]]::new()
    for ($i = 0; $i -lt $pkg.Count; $i++) {
        if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
            $bareArgs.Add('--package')
            $bareArgs.Add(($pkg[$i + 1] -split '@', 2)[0])
            $i++
            continue
        }
        $bareArgs.Add($pkg[$i])
    }
    # `cargo bolero list` emits one JSON object per target on stdout;
    # we want the set of (package, test) pairs over the affected set.
    $targetsJson = & cargo bolero list @bareArgs 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host 'anvil-bolero: bolero list failed; assuming no targets'
        exit 0
    }
    $targets = @($targetsJson | Where-Object { $_ -match '^\{' } | ForEach-Object { ConvertFrom-Json $_ })
    if ($targets.Count -eq 0) {
        Write-Host 'anvil-bolero: no fuzz targets in affected packages; skipping'
        exit 0
    }
    $failed = $false
    foreach ($t in $targets) {
        Write-Host "anvil-bolero: $($t.package_name)::$($t.test_name)"
        & cargo '+{{ rust_nightly }}' bolero test --release --engine libfuzzer -T 60s --package $t.package_name $t.test_name
        if ($LASTEXITCODE -ne 0) { $failed = $true }
    }
    if ($failed) { exit 1 }

# bolero needs cargo-bolero plus the pinned nightly toolchain
# (libfuzzer engine requires nightly rustc features).

# Install prerequisites for the `anvil-bolero` recipe.
[group("anvil-setup")]
anvil-bolero-setup installer="install": anvil-toolchain-nightly-install (anvil-tool-cargo-bolero-install installer)

# Validate prerequisites for the `anvil-bolero` recipe.
[group("anvil-setup")]
anvil-bolero-validate-prereqs: anvil-toolchain-nightly-validate-prereqs anvil-tool-cargo-bolero-validate-prereqs
