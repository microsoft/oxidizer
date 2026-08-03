# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Affected tier (compares changed crates' current API against the PR
# target branch baseline; only changed crates' surface is at risk).
#
# Several real-world conditions aren't actually SemVer violations:
#   - bin-only crates have no API to compare ("no library targets found").
#   - crates newly added since the PR base have no manifest at their
#     current workspace-relative path in that baseline.
#   - crates renamed since the PR base have no matching package in that
#     baseline ("package `<name>` not found in ...").
#   - crates where the branch baseline lacks a lib target the
#     current source has (bin -> bin+lib transition).
# We pre-filter to library-bearing crates from cargo metadata, skip
# packages whose current manifest path is absent from the baseline,
# then run cargo-semver-checks per-package and tolerate the remaining
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
# set, verify the PR branch baseline is present, skip newly-added
# manifest paths, then invoke per-crate against that baseline with
# selective error tolerance for renamed crates ("package `<name>` not
# found in ...") and bin->bin+lib transitions ("no library targets
# found").

# Check affected crates for semantic versioning violations.
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
    # Build the candidate package list. Keep each package's metadata so
    # its manifest path can also be checked in the baseline. Always
    # iterate per-package over library crates only -- cargo-semver-checks
    # --workspace would fail on workspaces that contain bin-only crates
    # ("no library targets found"), and we want the same tolerance for
    # new / renamed / bin->lib-transition crates regardless of whether
    # we got here via impact-scoping (cloud workflows) or full-workspace
    # fallback (local).
    $libPkgs = @{}
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    foreach ($p in $meta.packages) {
        if ($p.targets | Where-Object { $_.kind -contains 'lib' }) {
            $libPkgs[$p.name] = $p
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

    # _anvil-base-ref is the single source of truth for the baseline:
    # the PR's target branch on CI (origin/<target>), or origin/main /
    # origin/master locally. We compare the PR's *current* API against
    # that branch rather than the last crates.io release, so the recipe
    # reports what THIS PR changes -- not the accumulated delta since the
    # last publish.
    #
    # The jobs that run this recipe check out full history (GitHub
    # pr-fast: fetch-depth: 0; ADO: the default full checkout), so the
    # baseline commit is already present locally and no fetch is needed:
    # cargo-semver-checks reads only that one commit's tree (via gix, no
    # history walk). If the baseline is genuinely absent (a shallow local
    # clone, or a mis-configured job) we fail loudly with actionable
    # guidance rather than silently misreporting an unresolvable baseline
    # as a breaking change.
    $base = (& "{{just_executable()}}" _anvil-base-ref)
    $base = $base.Trim()
    & git rev-parse --verify "$base^{commit}" 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Error "anvil-semver-check: baseline '$base' is not present in this clone (shallow/truncated history? fetch it or unshallow)."
        exit 1
    }
    $findings = New-Object System.Collections.Generic.List[string]
    foreach ($p in $packages) {
        # A manifest absent at its current workspace-relative path did
        # not exist on the PR base, so this is a newly-added crate (or a
        # moved crate with no same-path baseline). Treat it as having no
        # comparable path baseline, so cargo-semver-checks need not run.
        $manifestPath = [System.IO.Path]::GetRelativePath(
            $meta.workspace_root,
            $libPkgs[$p].manifest_path
        ).Replace('\', '/')
        $baselineManifest = '{0}:{1}' -f $base, $manifestPath
        & git cat-file -e $baselineManifest 2>$null
        if ($LASTEXITCODE -ne 0) {
            Write-Host "anvil-semver-check: $p has no manifest at '$manifestPath' in baseline $base; skipping (new or moved crate)" -ForegroundColor Yellow
            continue
        }

        Write-Host "anvil-semver-check: $p (baseline $base)"
        $output = (& cargo semver-checks --package $p --baseline-rev $base 2>&1) | Out-String
        if ($LASTEXITCODE -ne 0) {
            # A crate renamed in place has a manifest at the same path but
            # no matching package name in the branch baseline ("package
            # `<name>` not found in <path>"). Tolerate that -- but NOT
            # when cargo-semver-checks appends "possibly due to errors:",
            # which means the baseline manifest failed to parse (a real
            # problem to surface, not a missing crate).
            $packageMissing = $output.Contains(('package `{0}` not found in' -f $p))
            $manifestParseFailed = $output.Contains('possibly due to errors:')
            $renamedPackage = $packageMissing -and -not $manifestParseFailed
            # The candidate filter above proves the current package has a
            # `lib` target, but the corresponding baseline target's `kind`
            # may not contain `lib` (a bin->lib transition).
            $baselineLibraryMissing = $output.Contains('no library targets found')
            if ($renamedPackage -or $baselineLibraryMissing) {
                Write-Host "  $p has no comparable branch baseline; skipping (likely renamed or bin->lib transition)" -ForegroundColor Yellow
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

# Install prerequisites for the `anvil-semver-check` recipe.
[group("anvil-setup")]
anvil-semver-check-setup installer="install": (anvil-tool-cargo-semver-checks-install installer)

# Validate prerequisites for the `anvil-semver-check` recipe.
[group("anvil-setup")]
anvil-semver-check-validate-prereqs: anvil-tool-cargo-semver-checks-validate-prereqs
