#!/usr/bin/env pwsh
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates Rust toolchain versions in rust-toolchain.toml and constants.env.

.DESCRIPTION
    This script automatically updates the Rust toolchain configuration to the latest versions:
    - Queries the latest stable Rust version and updates rust-toolchain.toml and RUST_LATEST in constants.env
    - Calculates yesterday's nightly build date and updates RUST_NIGHTLY in constants.env
    - Fetches the latest cargo-check-external-types release to determine the tested nightly version for RUST_NIGHTLY_EXTERNAL_TYPES

.PARAMETER ConstantsFile
    Path to the constants.env file. Defaults to ../constants.env relative to script location.

.PARAMETER ToolchainFile
    Path to the rust-toolchain.toml file. Defaults to ../rust-toolchain.toml relative to script location.

.PARAMETER DryRun
    If specified, shows what would be updated without actually modifying the files.

.EXAMPLE
    .\update_rust_toolchain.ps1

.EXAMPLE
    .\update_rust_toolchain.ps1 -DryRun
#>

param(
    [string]$ConstantsFile = (Join-Path $PSScriptRoot ".." "constants.env"),
    [string]$ToolchainFile = (Join-Path $PSScriptRoot ".." "rust-toolchain.toml"),
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

function Get-LatestStableRustVersion {
    <#
    .SYNOPSIS
        Retrieves the latest stable Rust version from the Rust GitHub releases.
    #>
    $apiUrl = "https://api.github.com/repos/rust-lang/rust/releases"

    try {
        $headers = @{
            "User-Agent" = "oxidizer-version-updater (github.com/microsoft/oxidizer)"
            "Accept" = "application/vnd.github+json"
        }

        $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop

        # Find the latest stable release (not a beta or nightly)
        $stableRelease = $response | Where-Object {
            $_.tag_name -match '^\d+\.\d+\.\d+$' -and -not $_.prerelease
        } | Select-Object -First 1

        if ($null -eq $stableRelease) {
            throw "No stable release found"
        }

        # Extract major.minor version (e.g., "1.92.0" -> "1.92")
        if ($stableRelease.tag_name -match '^(\d+\.\d+)\.\d+$') {
            $version = $Matches[1]
            return @{
                Success = $true
                Version = $version
                FullVersion = $stableRelease.tag_name
            }
        }
        else {
            throw "Could not parse version from tag: $($stableRelease.tag_name)"
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

function Update-RustToolchainToml {
    param(
        [string]$FilePath,
        [string]$NewVersion
    )

    $content = Get-Content $FilePath -Raw

    # Update the channel line
    $updatedContent = $content -replace '(?m)^channel = "[\d.]+"', "channel = `"$NewVersion`""

    return $updatedContent
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

# Main script execution
Write-Host "Fetching latest Rust versions..."
Write-Host ""

# Validate files exist
if (-not (Test-Path $ConstantsFile)) {
    Write-Host "Error: Constants file not found at '$ConstantsFile'"
    exit 1
}

if (-not (Test-Path $ToolchainFile)) {
    Write-Host "Error: Toolchain file not found at '$ToolchainFile'"
    exit 1
}

# Get current versions
$constantsContent = Get-Content $ConstantsFile
$toolchainContent = Get-Content $ToolchainFile -Raw

$currentRustLatest = ($constantsContent | Select-String '^RUST_LATEST=(.+)$').Matches.Groups[1].Value
$currentRustNightly = ($constantsContent | Select-String '^RUST_NIGHTLY=(.+)$').Matches.Groups[1].Value
$currentRustNightlyExternal = ($constantsContent | Select-String '^RUST_NIGHTLY_EXTERNAL_TYPES=(.+)$').Matches.Groups[1].Value
$currentToolchainVersion = if ($toolchainContent -match '(?m)^channel = "(.+)"') { $Matches[1] } else { "unknown" }

# Fetch new versions
$stableResult = Get-LatestStableRustVersion

if (-not $stableResult.Success) {
    Write-Host "Error fetching latest stable Rust version: $($stableResult.Error)"
    exit 1
}

$newStableVersion = $stableResult.Version
$yesterdayNightly = Get-YesterdayNightlyVersion
$externalTypesResult = Get-ExternalTypesTestedNightly

Write-Host "Current versions:"
Write-Host "  RUST_LATEST                : $currentRustLatest"
Write-Host "  RUST_NIGHTLY               : $currentRustNightly"
Write-Host "  RUST_NIGHTLY_EXTERNAL_TYPES: $currentRustNightlyExternal"
Write-Host ""

Write-Host "New versions:"
Write-Host "  RUST_LATEST                : $newStableVersion (full: $($stableResult.FullVersion))"
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
$toolchainNeedsUpdate = $false
$constantsUpdates = @{}

if ($currentToolchainVersion -ne $newStableVersion) {
    $toolchainNeedsUpdate = $true
}

if ($currentRustLatest -ne $newStableVersion) {
    $constantsUpdates["RUST_LATEST"] = $newStableVersion
}

if ($currentRustNightly -ne $yesterdayNightly) {
    $constantsUpdates["RUST_NIGHTLY"] = $yesterdayNightly
}

if ($externalTypesResult.Success -and $currentRustNightlyExternal -ne $externalTypesResult.Version) {
    $constantsUpdates["RUST_NIGHTLY_EXTERNAL_TYPES"] = $externalTypesResult.Version
}

if (-not $toolchainNeedsUpdate -and $constantsUpdates.Count -eq 0) {
    exit 0
}

if ($DryRun) {
    exit 0
}

# Apply updates
if ($toolchainNeedsUpdate) {
    $newToolchainContent = Update-RustToolchainToml -FilePath $ToolchainFile -NewVersion $newStableVersion
    Set-Content -Path $ToolchainFile -Value $newToolchainContent -NoNewline
}

if ($constantsUpdates.Count -gt 0) {
    $newConstantsContent = Update-ConstantsEnv -FilePath $ConstantsFile -Updates $constantsUpdates
    Set-Content -Path $ConstantsFile -Value $newConstantsContent -NoNewline
}
