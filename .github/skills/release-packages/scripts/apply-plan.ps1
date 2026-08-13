# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Applies a resolved release plan atomically.

.DESCRIPTION
    Updates package and workspace dependency versions, generates changelogs and
    READMEs, validates the workspace, and restores every touched file on failure.

.PARAMETER RepoRoot
    Workspace root containing Cargo.toml.

.PARAMETER PlanPath
    JSON emitted by resolve-plan.ps1.

.PARAMETER SkipReadme
    Skips `just readme`. Intended for synthetic workspaces that do not generate
    crate READMEs.
#>
[CmdletBinding()]
param(
    [string]$RepoRoot,

    [Parameter(Mandatory = $true)]
    [string]$PlanPath,

    [switch]$SkipReadme
)

$ErrorActionPreference = 'Stop'

$skillRepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = $skillRepoRoot
} else {
    $RepoRoot = (Resolve-Path $RepoRoot).Path
}

$changelogScript = Join-Path $PSScriptRoot 'release-changelog.ps1'
$rootManifest = Join-Path $RepoRoot 'Cargo.toml'
if (-not (Test-Path -LiteralPath $rootManifest)) {
    throw "Repository root '$RepoRoot' does not contain Cargo.toml."
}

$plan = Get-Content -LiteralPath (Resolve-Path $PlanPath) -Raw | ConvertFrom-Json
$releases = @($plan.releases)
if ($releases.Count -eq 0) {
    throw 'The release plan contains no packages.'
}

function Set-Utf8Content {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Content
    )

    [System.IO.File]::WriteAllText(
        $Path,
        $Content,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Set-PackageManifestVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedVersion,
        [Parameter(Mandatory = $true)][string]$Version
    )

    $content = Get-Content -LiteralPath $Path -Raw
    $packageSection = [regex]::Match(
        $content,
        '(?ms)^\[package\]\s*$.*?(?=^\[|\z)'
    )
    if (-not $packageSection.Success) {
        throw "Manifest '$Path' has no [package] section."
    }

    $matches = [regex]::Matches(
        $packageSection.Value,
        '(?m)^(?<prefix>\s*version\s*=\s*")(?<value>[^"]+)(?<suffix>".*)$'
    )
    if ($matches.Count -ne 1) {
        throw "Manifest '$Path' must contain exactly one literal [package] version."
    }
    if ($matches[0].Groups['value'].Value -ne $ExpectedVersion) {
        throw "Manifest '$Path' is at '$($matches[0].Groups['value'].Value)', not planned version '$ExpectedVersion'."
    }

    $updatedSection = $packageSection.Value.Remove(
        $matches[0].Groups['value'].Index,
        $matches[0].Groups['value'].Length
    ).Insert($matches[0].Groups['value'].Index, $Version)
    $updated = $content.Remove(
        $packageSection.Index,
        $packageSection.Length
    ).Insert($packageSection.Index, $updatedSection)
    Set-Utf8Content -Path $Path -Content $updated
}

function Set-WorkspaceDependencyVersion {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$ExpectedVersion,
        [Parameter(Mandatory = $true)][string]$Version
    )

    $content = Get-Content -LiteralPath $Path -Raw
    $section = [regex]::Match(
        $content,
        '(?ms)^\[workspace\.dependencies\]\s*$.*?(?=^\[|\z)'
    )
    if (-not $section.Success) {
        throw "Manifest '$Path' has no [workspace.dependencies] section."
    }

    $key = [regex]::Escape($Name)
    $entryPattern = "(?m)^(?<prefix>\s*(?:$key|`"$key`")\s*=\s*\{[^\r\n]*?\bversion\s*=\s*`")(?<value>[^`"]+)(?<suffix>`"[^\r\n]*\}\s*)$"
    $matches = [regex]::Matches($section.Value, $entryPattern)
    if ($matches.Count -ne 1) {
        throw "Workspace dependency '$Name' must be one inline table with a version value."
    }
    if ($matches[0].Groups['value'].Value -ne $ExpectedVersion) {
        throw "Workspace dependency '$Name' is at '$($matches[0].Groups['value'].Value)', not planned version '$ExpectedVersion'."
    }

    $updatedSection = $section.Value.Remove(
        $matches[0].Groups['value'].Index,
        $matches[0].Groups['value'].Length
    ).Insert($matches[0].Groups['value'].Index, $Version)
    $updated = $content.Remove(
        $section.Index,
        $section.Length
    ).Insert($section.Index, $updatedSection)
    Set-Utf8Content -Path $Path -Content $updated
}

function Invoke-CheckedCommand {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )

    $oldNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
    $PSNativeCommandUseErrorActionPreference = $false
    try {
        $output = & $Command @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    } finally {
        $PSNativeCommandUseErrorActionPreference = $oldNativeErrorPreference
    }
    if ($exitCode -ne 0) {
        $detail = ($output | Out-String).Trim()
        throw "Command failed: $Command $($Arguments -join ' ')`n$detail"
    }
}

$snapshot = @{}
function Save-OriginalFile {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ($snapshot.ContainsKey($Path)) { return }
    $snapshot[$Path] = if (Test-Path -LiteralPath $Path) {
        [pscustomobject]@{
            Exists = $true
            Bytes  = [System.IO.File]::ReadAllBytes($Path)
        }
    } else {
        [pscustomobject]@{
            Exists = $false
            Bytes  = $null
        }
    }
}

Save-OriginalFile -Path $rootManifest
Save-OriginalFile -Path (Join-Path $RepoRoot 'Cargo.lock')
foreach ($release in $releases) {
    Save-OriginalFile -Path (Join-Path $RepoRoot "crates\$($release.folder)\Cargo.toml")
    Save-OriginalFile -Path (Join-Path $RepoRoot "crates\$($release.folder)\CHANGELOG.md")
}
if (-not $SkipReadme) {
    Get-ChildItem -Path (Join-Path $RepoRoot 'crates') -Directory |
        ForEach-Object { Save-OriginalFile -Path (Join-Path $_.FullName 'README.md') }
}

try {
    foreach ($release in $releases) {
        $packageManifest = Join-Path $RepoRoot "crates\$($release.folder)\Cargo.toml"
        if (-not (Test-Path -LiteralPath $packageManifest)) {
            throw "Package '$($release.folder)' was not found under '$RepoRoot\crates'."
        }

        Set-PackageManifestVersion `
            -Path $packageManifest `
            -ExpectedVersion $release.from `
            -Version $release.to
        Set-WorkspaceDependencyVersion `
            -Path $rootManifest `
            -Name $release.name `
            -ExpectedVersion $release.from `
            -Version $release.to

        $reasonsJson = @($release.cascadeReasons) | ConvertTo-Json -Depth 5 -Compress
        & $changelogScript `
            -RepoRoot $RepoRoot `
            -PackageFolder $release.folder `
            -PackageName $release.name `
            -NewVersion $release.to `
            -PrBaseUrl 'https://github.com/microsoft/oxidizer' `
            -CascadeReasonsJson $reasonsJson
        if (-not $?) {
            throw "Changelog generation failed for '$($release.folder)'."
        }
    }

    if (-not $SkipReadme) {
        Invoke-CheckedCommand -Command just -Arguments @('readme')
    }
    Invoke-CheckedCommand -Command cargo -Arguments @(
        'metadata', '--manifest-path', $rootManifest, '--format-version', '1'
    )
    Invoke-CheckedCommand -Command cargo -Arguments @(
        'check', '--manifest-path', $rootManifest, '--workspace', '--all-features'
    )

    $oldNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
    $PSNativeCommandUseErrorActionPreference = $false
    try {
        $metadataOutput = & cargo metadata `
            --manifest-path $rootManifest `
            --format-version 1 `
            --no-deps
        $metadataExitCode = $LASTEXITCODE
    } finally {
        $PSNativeCommandUseErrorActionPreference = $oldNativeErrorPreference
    }
    if ($metadataExitCode -ne 0) {
        throw 'Failed to verify package versions with cargo metadata.'
    }
    $metadata = $metadataOutput | ConvertFrom-Json
    foreach ($release in $releases) {
        $package = @($metadata.packages | Where-Object name -eq $release.name)
        if ($package.Count -ne 1 -or $package[0].version -ne $release.to) {
            throw "Applied version for '$($release.name)' does not match '$($release.to)'."
        }

        $rootContent = Get-Content -LiteralPath $rootManifest -Raw
        $key = [regex]::Escape($release.name)
        $requirement = [regex]::Match(
            $rootContent,
            "(?m)^\s*(?:$key|`"$key`")\s*=\s*\{[^\r\n]*?\bversion\s*=\s*`"(?<value>[^`"]+)`""
        )
        if (-not $requirement.Success -or $requirement.Groups['value'].Value -ne $release.to) {
            throw "Workspace dependency version for '$($release.name)' does not match '$($release.to)'."
        }
    }
} catch {
    foreach ($path in $snapshot.Keys) {
        $original = $snapshot[$path]
        if ($original.Exists) {
            [System.IO.File]::WriteAllBytes($path, $original.Bytes)
        } elseif (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Force
        }
    }
    throw
}

[ordered]@{
    applied = @($releases | ForEach-Object { $_.folder })
} | ConvertTo-Json -Depth 3
