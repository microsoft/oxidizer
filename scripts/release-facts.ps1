# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Emits the deterministic release "facts" for the workspace as JSON.

.DESCRIPTION
    This is a small, deterministic helper for the AI release skill
    (.github/prompts/release-packages.prompt.md). It does NOT make any release
    decisions and NEVER writes to the repository. It only gathers the objective
    facts an agent needs to plan a release, so that different reasoning models
    start from an identical, machine-checked fact base rather than re-deriving it
    (and possibly diverging) by hand-parsing `cargo metadata` and `git` output.

    The facts are read from the existing, tested release library
    (scripts/lib/releasing.ps1) so this script stays a thin shell:

      - Get-WorkspacePackages          -> folder / name / version / published /
                                          proc-macro-only / library-target / deps
                                          (normal + build deps, dev excluded,
                                           names normalised with '-' -> '_').
      - Get-PreviousVersionBumpCommit  -> baseline commit sha for
                                          cargo-semver-checks (--baseline-rev).
      - Get-PackagesWithUnreleasedChanges -> which packages have unreleased
                                          modifications under crates/<folder>/.

    Version-bump arithmetic, cascade resolution, change-type classification, and
    all file writes are intentionally NOT done here -- those belong to the skill
    (judgment + planning) and to cargo-semver-checks / release-changelog.ps1.

.PARAMETER RepoRoot
    Workspace root (directory containing the top-level Cargo.toml). Defaults to
    the repository this script lives in.

.PARAMETER BaseRef
    Git ref used as the "previous release" boundary for baseline-commit lookup.
    Defaults to HEAD (the last committed version bump == the previous release).

.EXAMPLE
    ./scripts/release-facts.ps1 | ConvertFrom-Json
#>
[CmdletBinding()]
param(
    [string]$RepoRoot,
    [string]$BaseRef = 'HEAD'
)

$ErrorActionPreference = 'Stop'

. "$PSScriptRoot/lib/releasing.ps1"

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
} else {
    $RepoRoot = (Resolve-Path $RepoRoot).Path
}

# Start from a clean cache so repeated invocations (e.g. between mid-plan edits)
# never read stale cargo-metadata or git results.
Reset-ReleaseScriptCaches

$packages = @(Get-WorkspacePackages -repoRoot $RepoRoot)
$modified = Get-PackagesWithUnreleasedChanges -RepoRoot $RepoRoot

$factPackages = foreach ($package in $packages) {
    $baselineSha = $null
    # A brand-new crate (no prior version-bump commit) has no baseline and imposes
    # no change-type floor. Root-commit-only history can make the lookup throw; a
    # missing baseline is a fact, not a failure, so it degrades to $null.
    try {
        $bump = Get-PreviousVersionBumpCommit -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $package.Folder
        if ($null -ne $bump) { $baselineSha = $bump.Sha }
    } catch {
        $baselineSha = $null
    }

    [ordered]@{
        folder            = $package.Folder
        name              = $package.Name
        version           = $package.Version
        published         = [bool]$package.Published
        procMacroOnly     = [bool]$package.IsProcMacroOnly
        hasLibraryTarget  = [bool]$package.HasLibraryTarget
        deps              = @($package.Deps)
        baselineSha       = $baselineSha
        hasBaseline       = ($null -ne $baselineSha)
        modified          = $modified.ContainsKey($package.Folder)
        modifiedFileCount = if ($modified.ContainsKey($package.Folder)) { [int]$modified[$package.Folder] } else { 0 }
    }
}

[ordered]@{
    repoRoot = $RepoRoot
    baseRef  = $BaseRef
    packages = @($factPackages)
} | ConvertTo-Json -Depth 6
