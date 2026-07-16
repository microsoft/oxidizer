# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Affected tier (compares published API of changed crates against the
# baseline; only changed crates' surface is at risk).
#
# Several real-world conditions produce errors that aren't actually
# SemVer violations:
#   - bin-only crates have no API to compare ("no library targets found").
#   - crates not yet published to crates.io ("not found in registry").
#   - crates where the published baseline lacks a lib target the
#     current source has (bin -> bin+lib transition).
# We pre-filter to library-bearing crates from cargo metadata, then
# run cargo-semver-checks per-package and tolerate the
# "no-comparable-baseline" failure modes.
#
# Findings policy: this recipe is *advisory*. Real SemVer findings do
# NOT fail the recipe -- breaking changes between unreleased commits
# are normal (the major-version bump happens at release time, not on
# every PR). Instead, when there are findings we write a markdown
# advisory body to `target/anvil/comments/semver.md`; when the
# tree is clean we remove that file. cloud-workflow wiring (GH:
# marocchino/sticky-pull-request-comment; ADO: pwsh + REST API)
# inspects the file after the recipe and upserts / clears a sticky
# PR comment accordingly. Local invocation gets the same file
# written under target/ for inspection.
#
# TODO(anvil-runner): even after a `cargo ox-run` helper absorbs the
# skip/splat preamble, this recipe stays multi-step: cargo-metadata
# filter to library crates (cargo-semver-checks --workspace fails on
# bin-only workspaces), intersect ANVIL_INCLUDE_AFFECTED with that
# set, then per-crate invocation with selective error tolerance for
# unpublished crates ("not found in registry") and bin->bin+lib
# transitions ("no library targets found").
[script("pwsh")]
anvil-semver-check: anvil-semver-check-validate-prereqs
    $ErrorActionPreference = 'Stop'
    $commentFile = 'target/anvil/comments/semver.md'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-semver-check: no affected packages; skipping'
        Remove-Item -LiteralPath $commentFile -ErrorAction SilentlyContinue
        exit 0
    }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Build the candidate package list. Always iterate per-package over
    # library crates only -- cargo-semver-checks --workspace would fail
    # on workspaces that contain bin-only crates ("no library targets
    # found"), and we want the same tolerance for unpublished / bin->lib-
    # transition crates regardless of whether we got here via impact-
    # scoping (cloud workflows) or full-workspace fallback (local).
    $libPkgs = @{}
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    foreach ($p in $meta.packages) {
        if ($p.targets | Where-Object { $_.kind -contains 'lib' }) {
            $libPkgs[$p.name] = $true
        }
    }
    if ($pkg -contains '--workspace') {
        $packages = @($libPkgs.Keys)
    } else {
        $packages = @()
        for ($i = 0; $i -lt $pkg.Count; $i++) {
            if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
                # Impact emits version-qualified specs (name@version);
                # strip the @version to recover the bare package name.
                $name = ($pkg[$i + 1] -split '@', 2)[0]
                if ($libPkgs[$name]) { $packages += $name }
                $i++
            }
        }
    }
    if ($packages.Count -eq 0) {
        Write-Host 'anvil-semver-check: no affected library crates; skipping'
        Remove-Item -LiteralPath $commentFile -ErrorAction SilentlyContinue
        exit 0
    }
    $findings = New-Object System.Collections.Generic.List[string]
    foreach ($p in $packages) {
        Write-Host "anvil-semver-check: $p"
        $output = (& cargo semver-checks --package $p 2>&1) | Out-String
        if ($LASTEXITCODE -ne 0) {
            if ($output -match 'not found in registry|no library targets found') {
                Write-Host "  $p has no comparable baseline; skipping (likely unpublished or bin->lib transition)" -ForegroundColor Yellow
            } else {
                Write-Host $output
                # Append a per-crate findings block. Using one-line-at-a-time
                # appends keeps the markdown free of pwsh backtick-escape
                # gymnastics (single-quoted literals + the natural `n join
                # produce clean LF newlines and unambiguous triple-backticks).
                $findings.Add('### `' + $p + '`') | Out-Null
                $findings.Add('') | Out-Null
                $findings.Add('```') | Out-Null
                foreach ($line in ($output.TrimEnd() -split "`r?`n")) {
                    $findings.Add($line.TrimEnd()) | Out-Null
                }
                $findings.Add('```') | Out-Null
                $findings.Add('') | Out-Null
            }
        }
    }
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $commentFile)) | Out-Null
    if ($findings.Count -gt 0) {
        # Body starts with an HTML-comment marker so the ADO wiring can
        # locate the existing thread on subsequent runs (ADO has no
        # native "sticky comment header"; the marker is invisible to
        # human readers). Marocchino on GH uses its own `header:` input
        # and ignores the marker, but having it in the body keeps a
        # single source of truth across backends.
        $lines = New-Object System.Collections.Generic.List[string]
        $lines.Add('<!-- anvil-semver -->') | Out-Null
        $lines.Add('## :warning: Potential breaking changes detected') | Out-Null
        $lines.Add('') | Out-Null
        $lines.Add('`cargo semver-checks` flagged the following on this PR. This is **informational** -- breaking changes between commits are expected; the major-version bump happens at release time, not on every PR.') | Out-Null
        $lines.Add('') | Out-Null
        foreach ($f in $findings) { $lines.Add($f) | Out-Null }
        $body = ($lines -join "`n") + "`n"
        Set-Content -LiteralPath $commentFile -Value $body -Encoding UTF8 -NoNewline
        Write-Host ''
        Write-Host "anvil-semver-check: wrote advisory comment to $commentFile (recipe exits 0; cloud workflows will upsert a PR comment)" -ForegroundColor Yellow
    } else {
        Remove-Item -LiteralPath $commentFile -ErrorAction SilentlyContinue
    }
    # Advisory: always exit 0. cloud-workflow wiring posts/clears the PR comment.
    exit 0

[group("anvil-setup")]
anvil-semver-check-setup installer="install": (anvil-tool-cargo-semver-checks-install installer)

[group("anvil-setup")]
anvil-semver-check-validate-prereqs: anvil-tool-cargo-semver-checks-validate-prereqs
