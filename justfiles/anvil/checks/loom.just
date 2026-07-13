# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Affected tier. `loom` is a permutation-based concurrency model
# checker: a crate exercises it through a dedicated `[[test]]` target
# that declares `required-features = ["loom"]` (e.g. tests/loom.rs).
# We detect those targets *structurally* from `cargo metadata` -- a
# test target whose `kind` contains "test" and whose `required-features`
# contains "loom" -- so detection is authoritative manifest data, not a
# filename / cfg / comment heuristic. Running only those targets (via
# `--test <name>`) keeps loom off the crate's ordinary tests, which must
# not run under loom.
#
# Two independent switches must both be on, and they answer different
# questions:
#   * The `loom` feature answers "which target": `required-features`
#     makes Cargo select the loom test only when the feature is enabled,
#     so a normal `cargo test` / coverage run never compiles or runs it.
#     We pass --all-features so feature-gated loom sub-modules (e.g.
#     `#![cfg(all(loom, feature = "..."))]`) still compile.
#   * `RUSTFLAGS=--cfg loom` answers "how it compiles": loom's own
#     machinery and the ecosystem gate on `cfg(loom)`, not on a feature
#     (source swaps std<->loom atomics on `#[cfg(loom)]`, and
#     `[target.'cfg(loom)'.dependencies] loom` links only under the cfg).
#     The feature does NOT replace the cfg; both are required.
# We scope `cargo test` to the owning package with `-p` (never
# `--workspace`) so a global `--cfg loom` never leaks into deps reachable
# only through *other* members (e.g. concurrent-queue), and run
# single-threaded (loom drives its own scheduling). --release per the
# loom docs: permutation exploration is wall-clock dominated by the SUT.
#
# Fail-loud guard: a crate that *declares* loom support -- a `loom`
# feature or a `cfg(loom)`-gated dependency -- but exposes no
# `required-features = ["loom"]` test target is almost certainly a
# regression (renamed/removed target), so we error rather than silently
# skip it. When no crate ships a loom target (the common case) the recipe
# skips with exit 0 -- the regular tests already ran under anvil-llvm-cov.
#
# `cargo metadata --no-deps` scans only the adopter's own members
# (layout-agnostic; works for any `members` glob). RUSTFLAGS goes via
# $env: rather than --config so it propagates to cargo's child rustc
# invocations cleanly on both pwsh/bash.
[script("pwsh")]
anvil-loom: anvil-loom-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-loom: no affected packages; skipping'
        exit 0
    }
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    # Optional impact-scoping: recover bare package names from the
    # impact step's version-qualified `--package name@version` pairs.
    $affected = $null
    if ($env:ANVIL_INCLUDE_AFFECTED) {
        $tokens = @(-split $env:ANVIL_INCLUDE_AFFECTED)
        $affected = @()
        for ($i = 0; $i -lt $tokens.Count; $i++) {
            if ($tokens[$i] -eq '--package' -and ($i + 1) -lt $tokens.Count) {
                $affected += ($tokens[$i + 1] -split '@', 2)[0]
                $i++
            }
        }
    }
    $pkgs = $meta.packages
    if ($null -ne $affected) {
        $pkgs = @($pkgs | Where-Object { $_.name -in $affected })
    }

    # Authoritative detection: [[test]] targets with required-features = ["loom"].
    $loomTargets = @(foreach ($pkg in $pkgs) {
        foreach ($t in $pkg.targets) {
            if (($t.kind -contains 'test') -and ($t.'required-features' -contains 'loom')) {
                [pscustomobject]@{ pkg = $pkg.name; target = $t.name }
            }
        }
    })
    $loomTargetPkgs = @($loomTargets | ForEach-Object { $_.pkg } | Select-Object -Unique)

    # Fail loud: crates that declare loom support but ship no loom target.
    $declarers = @(foreach ($pkg in $pkgs) {
        $hasFeature = ($pkg.features.PSObject.Properties.Name -contains 'loom')
        $hasCfgDep = [bool]($pkg.dependencies | Where-Object { $_.target -and $_.target -match 'cfg\(.*\bloom\b.*\)' })
        if ($hasFeature -or $hasCfgDep) { $pkg.name }
    })
    $missing = @($declarers | Where-Object { $_ -notin $loomTargetPkgs } | Select-Object -Unique)
    if ($missing) {
        Write-Error ('anvil-loom: crate(s) declare loom support (a loom feature or a cfg(loom) dependency) but expose no [[test]] target with required-features = ["loom"]: ' + ($missing -join ', ') + '. Add a [[test]] target with required-features = ["loom"] (e.g. tests/loom.rs), or remove the loom feature / cfg(loom) dependency.')
        exit 1
    }

    if (-not $loomTargets) {
        Write-Host 'anvil-loom: no loom test targets; skipping'
        exit 0
    }

    $env:RUSTFLAGS = "--cfg loom $($env:RUSTFLAGS)".Trim()
    foreach ($lt in $loomTargets) {
        Write-Host "anvil-loom: $($lt.pkg) :: $($lt.target)"
        & cargo test -p $lt.pkg --release --all-features --locked --test $lt.target -- --test-threads=1
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

# loom is a regular cargo-test invocation with RUSTFLAGS=--cfg loom;
# no dedicated tool to install. The rust toolchain (rustc + cargo)
# is the only prerequisite.
[group("anvil-setup")]
anvil-loom-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-loom-validate-prereqs: anvil-tool-rustc-validate-prereqs
