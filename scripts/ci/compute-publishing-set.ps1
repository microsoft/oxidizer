# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Emits the set of workspace crates a PR is about to publish.

.DESCRIPTION
    A crate is "publishing" when its `[package] version = "..."` differs between
    the PR base ref and the working tree (a new crate not present at the base
    counts as publishing its first version). Only these crates are on a path to
    reach crates.io, so only these need a cargo-semver-checks compatibility check
    against their published baseline.

    Reuses `Get-PackagesWithVersionChanges` from the release library (the same
    per-crate version-diff logic the release scripts use), then maps the changed
    crate folders to their Cargo package names.

    Writes two GitHub Actions step outputs to the file named by -GitHubOutput
    (defaults to $env:GITHUB_OUTPUT):

      publishing = 'true' | 'false'
      packages   = '--package NAME --package NAME ...'   (empty when none)

.PARAMETER BaseRef
    The git ref to compare against, e.g. 'origin/main'. Must be fetched before
    calling this script.

.PARAMETER RepoRoot
    Repository root. Defaults to the current directory.

.PARAMETER GitHubOutput
    Path to the GitHub Actions step-output file. Defaults to $env:GITHUB_OUTPUT.
    When neither is set, the outputs are printed to stdout instead.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BaseRef,
    [string]$RepoRoot = (Get-Location).Path,
    [string]$GitHubOutput = $env:GITHUB_OUTPUT
)

. "$PSScriptRoot/../lib/releasing.ps1"

$changedFolders = Get-PackagesWithVersionChanges -RepoRoot $RepoRoot -BaseRef $BaseRef
$packages = Get-WorkspacePackages -repoRoot $RepoRoot
$nameByFolder = @{}
foreach ($p in $packages) { $nameByFolder[$p.Folder] = $p.Name }

$cargoNames = @(
    foreach ($folder in $changedFolders) {
        if ($nameByFolder.ContainsKey($folder)) { $nameByFolder[$folder] }
    }
) | Sort-Object -Unique

$pkgArgs = ($cargoNames | ForEach-Object { "--package $_" }) -join ' '
$publishing = if ($cargoNames.Count -gt 0) { 'true' } else { 'false' }

Write-Host "Publishing set: $(if ($pkgArgs) { $pkgArgs } else { '<none>' })"

$lines = @("publishing=$publishing", "packages=$pkgArgs")
if ([string]::IsNullOrEmpty($GitHubOutput)) {
    $lines | ForEach-Object { Write-Output $_ }
} else {
    $lines | Add-Content -Path $GitHubOutput -Encoding utf8
}
