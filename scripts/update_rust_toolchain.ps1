#!/usr/bin/env pwsh
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates Rust toolchain versions in constants.env and rust-toolchain.toml.

.DESCRIPTION
    This script automatically updates the Rust toolchain configuration to the latest versions:
    - Reads the internal stable Rust minor, then updates RUST_LATEST and rust-toolchain.toml to its latest upstream patch
    - Calculates yesterday's nightly build date and updates RUST_NIGHTLY in constants.env
    - Fetches the latest cargo-check-external-types release to determine the tested nightly version for RUST_NIGHTLY_EXTERNAL_TYPES

.PARAMETER ConstantsFile
    Path to the constants.env file. Defaults to ../constants.env relative to script location.

.PARAMETER ToolchainFile
    Path to the rust-toolchain.toml file. Defaults to ../rust-toolchain.toml relative to script location.

.PARAMETER InternalToolchainFile
    Path to ox-sdk's .pipelines/variables/publish.yml file. Update its MSRUSTUP_TOOLCHAIN values first;
    this script then aligns the public toolchain files to the latest upstream patch in that Rust minor.

.PARAMETER DryRun
    If specified, shows what would be updated without actually modifying the files.

.EXAMPLE
    .\update_rust_toolchain.ps1 -InternalToolchainFile ..\ox-sdk\.pipelines\variables\publish.yml

.EXAMPLE
    .\update_rust_toolchain.ps1 -InternalToolchainFile ..\ox-sdk\.pipelines\variables\publish.yml -DryRun
#>

param(
    [string]$ConstantsFile = (Join-Path $PSScriptRoot ".." "constants.env"),
    [string]$ToolchainFile = (Join-Path $PSScriptRoot ".." "rust-toolchain.toml"),
    [Parameter(Mandatory = $true)]
    [string]$InternalToolchainFile,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$ToolchainChannelPattern = '(?m)^([ \t]*channel[ \t]*=[ \t]*)"([^"]*)"'

function Get-LatestStableRustVersion {
    <#
    .SYNOPSIS
        Retrieves the latest stable Rust patch for a minor version from the Rust GitHub releases.
    #>
    param(
        [string]$MinorVersion
    )

    $apiUrl = "https://api.github.com/repos/rust-lang/rust/releases?per_page=100"

    try {
        $headers = @{
            "User-Agent" = "oxidizer-version-updater (github.com/microsoft/oxidizer)"
            "Accept" = "application/vnd.github+json"
        }

        $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop
        $versionPattern = "^$([regex]::Escape($MinorVersion))\.\d+$"

        # The internal production feed controls the usable minor; select its latest upstream patch.
        $stableRelease = $response | Where-Object {
            $_.tag_name -match $versionPattern -and -not $_.prerelease
        } | Select-Object -First 1

        if ($null -eq $stableRelease) {
            throw "No stable release found for Rust $MinorVersion"
        }

        return @{
            Success = $true
            Version = $stableRelease.tag_name
        }
    }
    catch {
        return @{
            Success = $false
            Error = $_.Exception.Message
        }
    }
}

function Get-YesterdayNightlyVersion {
    <#
    .SYNOPSIS
        Calculates the nightly version string for yesterday's date.
    #>
    $yesterday = (Get-Date).AddDays(-1)
    $nightlyVersion = "nightly-{0:yyyy-MM-dd}" -f $yesterday

    return $nightlyVersion
}

function Get-ExternalTypesTestedNightly {
    <#
    .SYNOPSIS
        Retrieves the latest tested nightly version from cargo-check-external-types releases.
    #>
    $apiUrl = "https://api.github.com/repos/awslabs/cargo-check-external-types/releases/latest"

    try {
        $headers = @{
            "User-Agent" = "oxidizer-version-updater (github.com/microsoft/oxidizer)"
            "Accept" = "application/vnd.github+json"
        }

        $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop

        # Parse the release notes to find the nightly version
        $body = $response.body

        # Look for patterns like "nightly-YYYY-MM-DD" in the release notes
        if ($body -match 'nightly-(\d{4}-\d{2}-\d{2})') {
            $nightlyVersion = "nightly-$($Matches[1])"
            return @{
                Success = $true
                Version = $nightlyVersion
                ReleaseVersion = $response.tag_name
            }
        }
        else {
            return @{
                Success = $false
                Error = "Could not find nightly version in release notes for $($response.tag_name)"
            }
        }
    }
    catch {
        return @{
            Success = $false
            Error = $_.Exception.Message
        }
    }
}

function Update-ConstantsEnv {
    param(
        [string]$FilePath,
        [hashtable]$Updates
    )

    $content = Get-Content $FilePath -Raw

    foreach ($key in $Updates.Keys) {
        $newValue = $Updates[$key]
        $pattern = "(?m)^$key=.+$"
        $replacement = "$key=$newValue"
        $content = $content -replace $pattern, $replacement
    }

    return $content
}

function Update-RustToolchain {
    param(
        [string]$FilePath,
        [string]$Version
    )

    $content = Get-Content $FilePath -Raw
    $replacement = '${1}"' + $Version + '"'

    return $content -replace $ToolchainChannelPattern, $replacement
}

function Get-InternalToolchainMinorVersion {
    param(
        [string]$FilePath
    )

    $content = Get-Content $FilePath -Raw
    $definition = [regex]::Match(
        $content,
        '(?ms)^[ \t]*-[ \t]+name:[ \t]*MSRUSTUP_TOOLCHAIN[ \t]*\r?\n(?<body>.*?)(?=^[ \t]*-[ \t]+name:|\z)'
    )

    if (-not $definition.Success) {
        throw "MSRUSTUP_TOOLCHAIN definition not found in '$FilePath'"
    }

    $values = [regex]::Matches($definition.Groups["body"].Value, '(?m)^[ \t]*value:[ \t]*(\S+)')

    if ($values.Count -eq 0) {
        throw "MSRUSTUP_TOOLCHAIN has no values in '$FilePath'"
    }

    $versions = @()

    foreach ($value in $values) {
        $channel = $value.Groups[1].Value
        $channelMatch = [regex]::Match(
            $channel,
            '^(?<quote>[''"]?)ms-prod-(?<minor>\d+\.\d+)(?:@[a-z0-9_-]+)?\k<quote>$'
        )

        if (-not $channelMatch.Success) {
            throw (
                "MSRUSTUP_TOOLCHAIN channel '$channel' in '$FilePath' has an invalid format. " +
                "Choose a production short name such as 'ms-prod-1.95' by running " +
                "'msrustup toolchain available'; append a required backend such as '@llvm' " +
                "only after the short name, for example 'ms-prod-1.95@llvm'."
            )
        }

        $versions += $channelMatch.Groups["minor"].Value
    }

    $uniqueVersions = @($versions | Sort-Object -Unique)

    if ($uniqueVersions.Count -ne 1) {
        throw "MSRUSTUP_TOOLCHAIN entries in '$FilePath' select different Rust versions: $($uniqueVersions -join ', ')"
    }

    return $uniqueVersions[0]
}

# Main script execution
Write-Host "Fetching latest Rust versions..."
Write-Host ""

# Validate files exist
if (-not (Test-Path $ConstantsFile)) {
    Write-Host "Error: Constants file not found at '$ConstantsFile'"
    exit 1
}

if (-not (Test-Path $ToolchainFile)) {
    Write-Host "Error: Rust toolchain file not found at '$ToolchainFile'"
    exit 1
}

if (-not (Test-Path $InternalToolchainFile)) {
    Write-Host "Error: Internal Rust toolchain file not found at '$InternalToolchainFile'"
    exit 1
}

# Get current versions
$constantsContent = Get-Content $ConstantsFile

$currentRustLatest = ($constantsContent | Select-String '^RUST_LATEST=(.+)$').Matches.Groups[1].Value
$currentRustNightly = ($constantsContent | Select-String '^RUST_NIGHTLY=(.+)$').Matches.Groups[1].Value
$currentRustNightlyExternal = ($constantsContent | Select-String '^RUST_NIGHTLY_EXTERNAL_TYPES=(.+)$').Matches.Groups[1].Value

$toolchainContent = Get-Content $ToolchainFile -Raw
$toolchainMatch = [regex]::Match($toolchainContent, $ToolchainChannelPattern)

if (-not $toolchainMatch.Success) {
    Write-Host "Error: Rust toolchain channel not found in '$ToolchainFile'"
    exit 1
}

$currentToolchain = $toolchainMatch.Groups[2].Value

try {
    $internalToolchainMinorVersion = Get-InternalToolchainMinorVersion -FilePath $InternalToolchainFile
}
catch {
    Write-Host "Error reading internal Rust toolchain version: $($_.Exception.Message)"
    exit 1
}

# Fetch new versions
$stableResult = Get-LatestStableRustVersion -MinorVersion $internalToolchainMinorVersion

if (-not $stableResult.Success) {
    Write-Host "Error fetching the latest stable Rust $internalToolchainMinorVersion release: $($stableResult.Error)"
    exit 1
}

$newStableVersion = $stableResult.Version
$yesterdayNightly = Get-YesterdayNightlyVersion
$externalTypesResult = Get-ExternalTypesTestedNightly

Write-Host "Current versions:"
Write-Host "  Internal Rust minor        : $internalToolchainMinorVersion"
Write-Host "  RUST_LATEST                : $currentRustLatest"
Write-Host "  rust-toolchain.toml channel: $currentToolchain"
Write-Host "  RUST_NIGHTLY               : $currentRustNightly"
Write-Host "  RUST_NIGHTLY_EXTERNAL_TYPES: $currentRustNightlyExternal"
Write-Host ""

Write-Host "New versions:"
Write-Host "  RUST_LATEST                : $newStableVersion"
Write-Host "  rust-toolchain.toml channel: $newStableVersion"
Write-Host "  RUST_NIGHTLY               : $yesterdayNightly"

if ($externalTypesResult.Success) {
    Write-Host "  RUST_NIGHTLY_EXTERNAL_TYPES: $($externalTypesResult.Version) (from release $($externalTypesResult.ReleaseVersion))"
}
else {
    Write-Host "  RUST_NIGHTLY_EXTERNAL_TYPES: Could not determine - $($externalTypesResult.Error)"
    Write-Host "    Keeping current value: $currentRustNightlyExternal"
}

Write-Host ""

# Determine what needs updating
$constantsUpdates = @{}

if ($currentRustLatest -ne $newStableVersion) {
    $constantsUpdates["RUST_LATEST"] = $newStableVersion
}

if ($currentRustNightly -ne $yesterdayNightly) {
    $constantsUpdates["RUST_NIGHTLY"] = $yesterdayNightly
}

if ($externalTypesResult.Success -and $currentRustNightlyExternal -ne $externalTypesResult.Version) {
    $constantsUpdates["RUST_NIGHTLY_EXTERNAL_TYPES"] = $externalTypesResult.Version
}

$toolchainNeedsUpdate = $currentToolchain -ne $newStableVersion

if ($constantsUpdates.Count -eq 0 -and -not $toolchainNeedsUpdate) {
    exit 0
}

if ($DryRun) {
    exit 0
}

# Apply updates
if ($constantsUpdates.Count -gt 0) {
    $newConstantsContent = Update-ConstantsEnv -FilePath $ConstantsFile -Updates $constantsUpdates
    Set-Content -Path $ConstantsFile -Value $newConstantsContent -NoNewline
}

if ($toolchainNeedsUpdate) {
    $newToolchainContent = Update-RustToolchain -FilePath $ToolchainFile -Version $newStableVersion
    Set-Content -Path $ToolchainFile -Value $newToolchainContent -NoNewline
}
