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

# Validate BaseRef once, up front, for a single clear failure. An unresolvable or
# un-fetched ref cannot yield meaningful facts. Get-PreviousVersionBumpCommit runs
# the same Test-GitRef internally and already throws on a bad ref, so this is not
# guarding against silent swallowing -- it just fails once here rather than once
# per package.
if (-not (Test-GitRef -Ref $BaseRef -RepoRoot $RepoRoot)) {
    throw "Base ref '$BaseRef' could not be resolved in '$RepoRoot'. Ensure it is fetched (CI should checkout with fetch-depth: 0) and spelled correctly."
}

$packages = @(Get-WorkspacePackages -repoRoot $RepoRoot)
$modified = Get-PackagesWithUnreleasedChanges -RepoRoot $RepoRoot

$factPackages = foreach ($package in $packages) {
    $baselineSha = $null
    # baselineSha is the crate's previous version-bump commit, or null if none can
    # be found. Note a crate's introducing commit counts as a bump, so in practice
    # even a never-released crate gets a baselineSha (its own first commit) -- use
    # the everReleased fact below, NOT hasBaseline, to tell a first-ever release
    # apart from a real one.
    $bump = Get-PreviousVersionBumpCommit -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $package.Folder
    if ($null -ne $bump) { $baselineSha = $bump.Sha }

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
        # Whether the crate has ever been published, determined from its release
        # tags. A crate's introducing commit counts as a version bump, so
        # hasBaseline alone cannot distinguish a first-ever release from a real one;
        # cargo-semver-checks against an unpublished baseline would classify normal
        # pre-publication churn as breaking. The release skill's Step 3 branches on
        # this fact.
        everReleased      = [bool](Invoke-Git -Arguments @('tag', '--list', "$($package.Name)-v*") -RepoRoot $RepoRoot)
        modified          = $modified.ContainsKey($package.Folder)
        modifiedFileCount = if ($modified.ContainsKey($package.Folder)) { [int]$modified[$package.Folder] } else { 0 }
    }
}

[ordered]@{
    repoRoot = $RepoRoot
    baseRef  = $BaseRef
    packages = @($factPackages)
} | ConvertTo-Json -Depth 6
