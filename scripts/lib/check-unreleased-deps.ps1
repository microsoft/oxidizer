# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Library backing scripts/check-unreleased-dependencies.ps1. Holds the
    helpers and main flow so the wrapper script stays a thin CLI shim and the
    helpers stay individually testable via Pester.

.DESCRIPTION
    The wrapper script `scripts/check-unreleased-dependencies.ps1` dot-sources
    this file and calls `Invoke-CheckUnreleasedDependencies`. Tests dot-source
    this file directly to exercise the helpers without triggering the wrapper's
    main flow.

    See `docs/releasing.md` for the overall release-tooling vocabulary
    (release set, pending release, cascade toward dependents/dependencies,
    change type vs version component).
#>

. (Join-Path $PSScriptRoot 'releasing.ps1')

# Appends "$Name=$Value" to $env:GITHUB_OUTPUT, when defined. A no-op outside
# GitHub Actions so local invocations don't blow up.
function Set-StepOutput {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Value
    )

    if ([string]::IsNullOrEmpty($env:GITHUB_OUTPUT)) { return }
    Add-Content -Path $env:GITHUB_OUTPUT -Value "$Name=$Value"
}

# Returns the absolute repo root by asking git. Wrapped so tests can mock it.
function Get-RepoRoot {
    $output = Invoke-Git -Arguments @('rev-parse', '--show-toplevel')
    return ($output | Select-Object -First 1).ToString().Trim()
}

# Renders a dependency chain like ('a','b','c') as 'a -> b -> c'. A single
# element renders as that element. Pure.
function Format-DependencyChain {
    param([Parameter(Mandatory = $true)][string[]]$Chain)
    return ($Chain -join ' -> ')
}

# Renders one line of the "This PR releases the following workspace packages"
# bullet list, of the form:
#   - `<folder>` <base> -> <current>
# or, when the package isn't present at $BaseRef:
#   - `<folder>` <current> (new package)
# When the on-disk Cargo.toml is missing (extremely unusual; only happens if a
# release commit deletes a package mid-flow), the current version renders as
# `?`. The function does its own version lookups; tests exercise it against a
# real synthetic workspace.
function Format-ReleaseEntry {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef,
        [Parameter(Mandatory = $true)][string]$Folder
    )

    $cargo = Join-Path (Join-Path $RepoRoot 'crates') $Folder 'Cargo.toml'
    $current = if (Test-Path $cargo) { Get-CurrentVersion -cargoTomlPath $cargo } else { '?' }
    $base = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $Folder
    if ([string]::IsNullOrEmpty($base)) {
        return "  - ``$Folder`` $current (new package)"
    }
    return "  - ``$Folder`` $base -> $current"
}

# Pure formatter for the markdown comment body. Takes already-resolved data
# (release-set entries as rendered strings; findings split into two buckets)
# and returns the entire markdown body, LF-terminated. No I/O, no version
# lookups, no git calls — every dependency is in the parameter list.
#
# `ReleaseEntryLines` are the pre-rendered "  - `<folder>` X -> Y" strings
# from Format-ReleaseEntry, in display order. `NotReleasedFindings` and
# `ElevationCandidates` are arrays of finding objects with the shape
# emitted by Get-UnreleasedModifiedDependencies (Folder, ChangedFileCount,
# DependencyChains as an array-of-string-arrays). Either bucket may be
# empty; when both are empty the function still emits the release-set
# bullet list and the "What this means" footer (callers should guard
# upstream when there are no findings at all — that path doesn't write a
# comment in production).
function Format-UnreleasedDependenciesReport {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$ReleaseEntryLines,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$NotReleasedFindings,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$ElevationCandidates
    )

    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add('## 📦 Unreleased Workspace Dependency Changes') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('This PR releases the following workspace packages:') | Out-Null
    $lines.Add('') | Out-Null
    foreach ($entry in $ReleaseEntryLines) {
        $lines.Add($entry) | Out-Null
    }
    $lines.Add('') | Out-Null

    if ($NotReleasedFindings.Count -gt 0) {
        $lines.Add('The following workspace packages have **unreleased modifications** (changes newer than their last `version =` or `publish =` change) and are *not* part of this release:') | Out-Null
        $lines.Add('') | Out-Null
        $lines.Add('| Package | Files changed | Reached via |') | Out-Null
        $lines.Add('|-------|--------------:|-------------|') | Out-Null
        foreach ($finding in $NotReleasedFindings) {
            $chains = @($finding.DependencyChains | ForEach-Object { Format-DependencyChain -Chain $_ })
            $rendered = ($chains | Sort-Object -Unique | ForEach-Object { "``$_``" }) -join '<br>'
            $lines.Add("| ``$($finding.Folder)`` | $($finding.ChangedFileCount) | $rendered |") | Out-Null
        }
        $lines.Add('') | Out-Null
    }

    if ($ElevationCandidates.Count -gt 0) {
        $lines.Add('The following workspace packages **are** part of this release, but their change type is non-breaking / patch while they also contain modifications from earlier commits. Reviewer should confirm the chosen change type is appropriate:') | Out-Null
        $lines.Add('') | Out-Null
        $lines.Add('| Package | Files changed | Reached via |') | Out-Null
        $lines.Add('|-------|--------------:|-------------|') | Out-Null
        foreach ($finding in $ElevationCandidates) {
            $chains = @($finding.DependencyChains | ForEach-Object { Format-DependencyChain -Chain $_ })
            $rendered = ($chains | Sort-Object -Unique | ForEach-Object { "``$_``" }) -join '<br>'
            $lines.Add("| ``$($finding.Folder)`` | $($finding.ChangedFileCount) | $rendered |") | Out-Null
        }
        $lines.Add('') | Out-Null
    }
    $lines.Add('### What this means') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('Locally, the released packages build against the modified version of each unreleased dependency via path-references. Once published, however, they will resolve against the **last released** version of each dependency on crates.io — which does not include the unreleased changes.') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('- If the unreleased changes are **material** to the released package''s behavior or public API, you should release the dependency too (add it to the `-Packages` list when re-running ``scripts/release-packages.ps1``).') | Out-Null
    $lines.Add('- If the changes are **immaterial** (formatting, doc tweaks, internal-only refactors), this comment can be ignored.') | Out-Null
    $lines.Add('- For packages **already part of this release** that contain extra modifications, confirm that the change type chosen at release time (non-breaking / patch) is appropriate for the cumulative change set.') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('<sub>This is an automated informational check. It does not fail the build.</sub>') | Out-Null

    return ($lines -join "`n") + "`n"
}

# Encapsulates the main flow of the check script so the wrapper file stays a
# thin CLI shim and the catch path is reachable from tests without `exit`
# terminating the test session.
#
# Behavior contract:
#   - Bad base ref ⇒ warning, has_findings=false, no file written, return.
#   - No findings ⇒ "No modified-but-unreleased..." message, has_findings=false,
#     no file written, return.
#   - Findings ⇒ markdown comment written to -OutputFile, has_findings=true.
#   - Any caught exception ⇒ Write-Warning (NOT Write-Error — keeps test
#     callers safe under ErrorActionPreference=Stop), has_findings=false,
#     return. The wrapper script's `exit 0` still runs.
function Invoke-CheckUnreleasedDependencies {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $false)][string]$BaseRef = 'origin/main',
        [Parameter(Mandatory = $true)][string]$OutputFile
    )

    $ErrorActionPreference = 'Stop'

    try {
        $repoRoot = Get-RepoRoot

        if (-not (Test-GitRef -Ref $BaseRef -RepoRoot $repoRoot)) {
            Write-Warning "Base ref '$BaseRef' could not be resolved; skipping analysis."
            Set-StepOutput -Name 'has_findings' -Value 'false'
            return
        }

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $repoRoot -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $repoRoot -BaseRef $BaseRef))

        if ($findings.Count -eq 0) {
            Write-Host "No modified-but-unreleased workspace dependencies detected."
            Set-StepOutput -Name 'has_findings' -Value 'false'
            return
        }

        # Get-PackagesWithVersionChanges returns a HashSet via Write-Output -NoEnumerate so
        # callers can use .Contains() on it. That same wrapping defeats `Sort-Object`:
        # piping a NoEnumerate'd HashSet sends it as a single object, and the sort
        # becomes a no-op. Unwrap explicitly via ForEach-Object before sorting.
        $releaseSetHash = Get-PackagesWithVersionChanges -RepoRoot $repoRoot -BaseRef $BaseRef
        $releaseSet = @($releaseSetHash | ForEach-Object { $_ }) | Sort-Object

        $releaseEntryLines = @($releaseSet | ForEach-Object {
            Format-ReleaseEntry -RepoRoot $repoRoot -BaseRef $BaseRef -Folder $_
        })

        # Split findings into two reviewer-facing categories:
        #   - "not part of this release"       — modified dependency that is NOT in the release set.
        #   - "elevation candidates"           — modified dependency that IS in the release set, but
        #                                        its change type is non-breaking / patch (so the user
        #                                        may want to elevate after reviewing the diff).
        $sortedFindings    = @($findings | Sort-Object { $_.Folder })
        $notReleased        = @($sortedFindings | Where-Object { -not $_.InReleaseSet })
        $elevationCandidates = @($sortedFindings | Where-Object { $_.InReleaseSet })

        $content = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    $releaseEntryLines `
            -NotReleasedFindings  $notReleased `
            -ElevationCandidates  $elevationCandidates

        Set-Content -Path $OutputFile -Value $content -Encoding utf8 -NoNewline

        $findingCount = $findings.Count
        Write-Host "Wrote $findingCount finding$(if ($findingCount -eq 1) { '' } else { 's' }) to '$OutputFile'."
        Set-StepOutput -Name 'has_findings' -Value 'true'
        return
    }
    catch {
        # Don't block the PR on tool failures. Surface has_findings=false so no stale
        # comment is left posted. Write-Warning (not Write-Error) so callers with
        # ErrorActionPreference=Stop in their scope aren't terminated.
        Set-StepOutput -Name 'has_findings' -Value 'false'
        Write-Warning "check-unreleased-dependencies failed: $_"
        return
    }
}
