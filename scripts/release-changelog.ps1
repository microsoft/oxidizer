# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Regenerates one package's CHANGELOG.md for a release, deterministically.

.DESCRIPTION
    A small, deterministic helper for the AI release skill
    (.github/prompts/release-packages.prompt.md). Changelog generation is the
    kind of mechanical, format-heavy sub-task that an agent should NOT reproduce
    by hand -- grouping conventional commits into sections, rendering PR links,
    folding `## Unreleased`, and emitting cascade "Now requires X of Y" bullets in
    a stable order. Forwarding it to a script keeps the output byte-identical
    regardless of which reasoning model drove the plan.

    This is a thin shell over the existing, tested Write-Changelog function in
    scripts/lib/release-flow.ps1; it does not reimplement any changelog logic.
    It writes exactly one file: crates/<PackageFolder>/CHANGELOG.md.

.PARAMETER RepoRoot
    Workspace root (directory containing the top-level Cargo.toml). Defaults to
    the repository this script lives in.

.PARAMETER PackageFolder
    Folder name under crates/ for the package being released.

.PARAMETER NewVersion
    The already-decided target version (e.g. '1.3.0'). This helper performs NO
    version arithmetic -- the skill computes the version and passes it in.

.PARAMETER PrBaseUrl
    Base URL used to render PR links (e.g. https://github.com/microsoft/oxidizer).

.PARAMETER CascadeReasonsJson
    Optional JSON array describing why this package is being re-released because a
    dependency was released. Each element:
        { "Target": "<dep-name>", "Version": "<x.y.z>", "Breaking": false }
    Produces a "🔧 Maintenance" (or "⚠️ Breaking" if any reason is breaking)
    section with one "Now requires `<Version>` of `<Target>`" bullet per reason.

.EXAMPLE
    ./scripts/release-changelog.ps1 -PackageFolder bytesbuf_io -NewVersion 0.9.0 `
        -PrBaseUrl https://github.com/microsoft/oxidizer `
        -CascadeReasonsJson '[{"Target":"bytesbuf","Version":"0.9.0","Breaking":true}]'
#>
[CmdletBinding()]
param(
    [string]$RepoRoot,

    [Parameter(Mandatory = $true)]
    [string]$PackageFolder,

    [Parameter(Mandatory = $true)]
    [string]$NewVersion,

    [string]$PrBaseUrl,

    [string]$CascadeReasonsJson
)

$ErrorActionPreference = 'Stop'

. "$PSScriptRoot/lib/changelog.ps1"

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
} else {
    $RepoRoot = (Resolve-Path $RepoRoot).Path
}

Reset-ReleaseScriptCaches

$package = @(Get-WorkspacePackages -repoRoot $RepoRoot) |
    Where-Object { $_.Folder -eq $PackageFolder } |
    Select-Object -First 1
if ($null -eq $package) {
    throw "Package folder '$PackageFolder' was not found under 'crates/' in '$RepoRoot'."
}

$packageFolderPath = Join-Path $RepoRoot "crates/$PackageFolder"
$changelogFile     = Join-Path $packageFolderPath 'CHANGELOG.md'

$cascadeReasons = $null
if (-not [string]::IsNullOrWhiteSpace($CascadeReasonsJson)) {
    $cascadeReasons = @($CascadeReasonsJson | ConvertFrom-Json)
}

# Write-Changelog resolves git history relative to the current directory, so run
# it from the workspace root.
Push-Location $RepoRoot
try {
    Write-Changelog -packageName $package.Name `
        -newVersion $NewVersion `
        -packageFolder $packageFolderPath `
        -changelogFile $changelogFile `
        -prBaseUrl $PrBaseUrl `
        -cascadeReasons $cascadeReasons
} finally {
    Pop-Location
}
