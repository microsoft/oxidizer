# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Modified tier.
#
# cargo-doc2readme regenerates a crate's README.md from its rustdoc.
# Two extension points users frequently need:
#   * Workspace-level template (`crates/README.j2` or `README.j2` at repo
#     root): a Tera template applied to every crate's README. anvil
#     auto-detects it and passes `--template` to the per-crate runs.
#   * Per-crate opt-out: hand-crafted READMEs (e.g. a tool crate whose
#     README is more freeform than the lib docs) opt out by adding
#     `[package.metadata.ox-gen-readme]\ndisable = true` to their
#     Cargo.toml. anvil skips those crates.
#
# Bin-only crates have no library rustdoc to base a README on, so they
# are skipped as well (cargo doc2readme requires a library target).
# TODO(anvil-runner): even after a `cargo ox-run` helper absorbs the
# skip/splat preamble, this recipe stays multi-step: per-crate
# iteration over library targets, cargo-metadata-driven opt-outs
# (publish=false, [package.metadata.ox-gen-readme] disable), per-crate
# Push-Location into the crate dir (cargo-doc2readme is CWD-sensitive
# rather than --manifest-path-driven), and per-crate template-path
# resolution. The ANVIL_INCLUDE_MODIFIED value would still need to
# be intersected with the lib-crate set rather than splatted into cargo.

# Check that crate README files are up to date.
[script("pwsh")]
anvil-readme-check: anvil-readme-check-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') {
        Write-Host 'anvil-readme-check: no modified packages; skipping'
        exit 0
    }
    # Detect a workspace-level README template. Two conventional
    # locations: crates/README.j2 (cargo-workspaces idiom) or
    # README.j2 at repo root.
    $template = $null
    foreach ($candidate in 'crates/README.j2', 'README.j2') {
        if (Test-Path $candidate) { $template = (Resolve-Path $candidate).Path; break }
    }
    # Iterate library crates. Filter by impact set when set, then drop
    # bin-only crates and opt-outs.
    $pkg = @(if ($env:ANVIL_INCLUDE_MODIFIED) { -split $env:ANVIL_INCLUDE_MODIFIED } else { '--workspace' })
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $byName = @{}
    foreach ($p in $meta.packages) { $byName[$p.name] = $p }
    $candidates = if ($pkg -contains '--workspace') {
        @($meta.packages | ForEach-Object { $_.name })
    } else {
        $names = @()
        for ($i = 0; $i -lt $pkg.Count; $i++) {
            if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
                # Impact emits version-qualified specs (name@version);
                # strip the @version to recover the bare package name
                # this metadata lookup keys on.
                $names += ($pkg[$i + 1] -split '@', 2)[0]; $i++
            }
        }
        $names
    }
    $hadFailure = $false
    foreach ($name in $candidates) {
        $p = $byName[$name]
        if (-not $p) { continue }
        if (-not ($p.targets | Where-Object { $_.kind -contains 'lib' })) { continue }
        # Skip private crates (publish = false). They aren't released
        # and rarely have a polished README. Mirrors the
        # `cargo workspaces exec --ignore-private` idiom adopters
        # commonly use.
        if ($p.publish -is [array] -and $p.publish.Count -eq 0) {
            Write-Host "anvil-readme-check: $name (skipped: publish = false)"
            continue
        }
        $disabled = $false
        if ($p.metadata -and $p.metadata.'ox-gen-readme' -and $p.metadata.'ox-gen-readme'.disable) {
            $disabled = $true
        }
        if ($disabled) {
            Write-Host "anvil-readme-check: $name (opted out via [package.metadata.ox-gen-readme])"
            continue
        }
        Write-Host "anvil-readme-check: $name"
        # cargo doc2readme writes / compares relative to its CWD (not
        # --manifest-path), so chdir into the crate before invoking
        # --check. We also compute a per-crate relative path to the
        # workspace-level template so the same template file works for
        # every crate (parallels the cargo-workspaces idiom).
        $crateDir = Split-Path -Parent $p.manifest_path
        Push-Location $crateDir
        try {
            $relTemplate = if ($template) {
                Resolve-Path -Relative -LiteralPath $template
            } else {
                $null
            }
            $args = @('doc2readme', '--check')
            if ($relTemplate) { $args += @('--template', $relTemplate) }
            & cargo @args
            if ($LASTEXITCODE -ne 0) { $hadFailure = $true }
        } finally {
            Pop-Location
        }
    }
    if ($hadFailure) { exit 1 }

# Install prerequisites for the `anvil-readme-check` recipe.
[group("anvil-setup")]
anvil-readme-check-setup installer="install": (anvil-tool-cargo-doc2readme-install installer)

# Validate prerequisites for the `anvil-readme-check` recipe.
[group("anvil-setup")]
anvil-readme-check-validate-prereqs: anvil-tool-cargo-doc2readme-validate-prereqs
