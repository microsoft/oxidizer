# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Affected tier (lints public API of changed crates and rev-deps).
#
# cargo-check-external-types is per-manifest: no --package/--workspace,
# only --manifest-path. Iterate the affected library crates and run
# the tool once each, pointing at the crate's Cargo.toml. Bin-only
# crates have no public API surface and are skipped.
# TODO(anvil-runner): even after a `cargo ox-run` helper absorbs the
# skip/splat preamble, this recipe stays multi-step: cargo-check-
# external-types is per-manifest (no --package/--workspace), so we
# build a name->manifest map from cargo metadata, filter to lib crates,
# intersect with ANVIL_INCLUDE_AFFECTED, and call the tool once per
# crate. Hard-fails on errors (no tolerance, unlike semver-check).

# Check public APIs for unapproved external types.
[script("pwsh")]
anvil-external-types: anvil-external-types-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-external-types: no affected packages; skipping'
        exit 0
    }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Build pkg-name -> manifest-path map, restricted to library crates.
    $libPkgs = @{}
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    foreach ($p in $meta.packages) {
        if ($p.targets | Where-Object { $_.kind -contains 'lib' }) {
            $libPkgs[$p.name] = $p.manifest_path
        }
    }
    # Decide which packages to check.
    $packages = @()
    if ($pkg -contains '--workspace') {
        $packages = $libPkgs.Keys
    } else {
        for ($i = 0; $i -lt $pkg.Count; $i++) {
            if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
                # Impact emits version-qualified specs (name@version);
                # strip the @version to recover the bare package name
                # this manifest-path map keys on.
                $name = ($pkg[$i + 1] -split '@', 2)[0]
                if ($libPkgs.ContainsKey($name)) { $packages += $name }
                $i++
            }
        }
    }
    if ($packages.Count -eq 0) {
        Write-Host 'anvil-external-types: no affected library crates; skipping'
        exit 0
    }
    $failed = $false
    foreach ($p in $packages) {
        Write-Host "anvil-external-types: $p"
        # cargo-check-external-types requires nightly rustdoc (uses
        # unstable -Z flags) AND pins a specific rustdoc-types schema
        # version. We pin nightly narrowly to the schema this tool
        # version expects via `rust_nightly_external_types` in
        # versions.just — bump that pin alongside any cargo-check-
        # external-types upgrade. No tolerance for schema mismatches:
        # if it fails, the pin or the tool needs to move.
        & cargo '+{{ rust_nightly_external_types }}' check-external-types --manifest-path $libPkgs[$p]
        if ($LASTEXITCODE -ne 0) { $failed = $true }
    }
    if ($failed) { exit 1 }

# Install prerequisites for the `anvil-external-types` recipe.
[group("anvil-setup")]
anvil-external-types-setup installer="install": anvil-toolchain-nightly-external-types-install (anvil-tool-cargo-check-external-types-install installer)

# Validate prerequisites for the `anvil-external-types` recipe.
[group("anvil-setup")]
anvil-external-types-validate-prereqs: anvil-toolchain-nightly-external-types-validate-prereqs anvil-tool-cargo-check-external-types-validate-prereqs
