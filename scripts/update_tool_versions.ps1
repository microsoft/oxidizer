#!/usr/bin/env pwsh
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates cargo tool versions in constants.env to their latest stable versions from crates.io.

.DESCRIPTION
    This script reads constants.env, extracts all cargo tool version variables,
    queries the crates.io API for the latest stable version of each crate (excluding
    alpha/beta/prerelease versions), and updates the file with the latest versions.
    Rate limiting is applied (1 request per second) to respect crates.io's usage policies.

.PARAMETER ConstantsFile
    Path to the constants.env file. Defaults to ../constants.env relative to script location.

.PARAMETER DryRun
    If specified, shows what would be updated without actually modifying the file.

.EXAMPLE
    .\update_tool_versions.ps1

.EXAMPLE
    .\update_tool_versions.ps1 -DryRun
#>

param(
    [string]$ConstantsFile = (Join-Path $PSScriptRoot ".." "constants.env"),
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

function Get-CrateLatestVersion {
    param(
        [string]$CrateName
    )

    $apiUrl = "https://crates.io/api/v1/crates/$CrateName"

    try {
        $headers = @{
            "User-Agent" = "oxidizer-version-updater (github.com/microsoft/oxidizer)"
        }

        $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop

        # Use max_stable_version to exclude alpha/beta/prerelease versions
        # Fall back to max_version if max_stable_version is not available
        $latestVersion = if ($response.crate.max_stable_version) {
            $response.crate.max_stable_version
        } else {
            $response.crate.max_version
        }

        return @{
            Success = $true
            Version = $latestVersion
        }
    }
    catch {
        return @{
            Success = $false
            Error = $_.Exception.Message
        }
    }
}

function ConvertTo-CrateName {
    param(
        [string]$VariableName
    )

    # Map environment variable names to crate names
    # CARGO_FOO_BAR_VERSION -> cargo-foo-bar
    # JUST_VERSION -> just
    # SCCACHE_VERSION -> sccache

    $name = $VariableName -replace '_VERSION$', ''
    $name = $name.ToLower() -replace '_', '-'

    return $name
}

function Get-VersionPrefix {
    param(
        [string]$CurrentVersion
    )

    # Some tools use version prefixes like 'v' (e.g., sccache uses v0.12.0)
    if ($CurrentVersion -match '^v') {
        return 'v'
    }
    return ''
}

# Main script execution
if (-not (Test-Path $ConstantsFile)) {
    Write-Host "Error: Constants file not found at '$ConstantsFile'"
    exit 1
}

# Read the file content
$fileContent = Get-Content $ConstantsFile -Raw
$lines = Get-Content $ConstantsFile

# Find all version variables
$versionPattern = '^(CARGO_\w+_VERSION|JUST_VERSION|SCCACHE_VERSION)=(.+)$'
$updates = @()
$skipped = @()
$unchanged = @()

# First pass: collect all crate names to calculate padding
$tools = @()
foreach ($line in $lines) {
    if ($line -match $versionPattern) {
        $variableName = $Matches[1]
        $currentVersion = $Matches[2]
        $crateName = ConvertTo-CrateName -VariableName $variableName

        $tools += @{
            Variable = $variableName
            CurrentVersion = $currentVersion
            CrateName = $crateName
        }
    }
}

# Calculate maximum crate name length for padding
$maxLength = ($tools | ForEach-Object { $_.CrateName.Length } | Measure-Object -Maximum).Maximum

# Second pass: query and display results
foreach ($tool in $tools) {
    $variableName = $tool.Variable
    $currentVersion = $tool.CurrentVersion
    $crateName = $tool.CrateName
    $padding = " " * ($maxLength - $crateName.Length)

    # Query crates.io for the latest version
    $result = Get-CrateLatestVersion -CrateName $crateName

    if (-not $result.Success) {
        Write-Host "$crateName$padding`: $($result.Error)"
        $skipped += @{
            Variable = $variableName
            CrateName = $crateName
            CurrentVersion = $currentVersion
            Reason = $result.Error
        }
        Start-Sleep -Seconds 1
        continue
    }

    # Preserve version prefix if present (e.g., 'v' for sccache)
    $versionPrefix = Get-VersionPrefix -CurrentVersion $currentVersion
    $newVersion = "$versionPrefix$($result.Version)"

    if ($currentVersion -eq $newVersion) {
        Write-Host "$crateName$padding`: unchanged, $currentVersion is the latest version"
        $unchanged += @{
            Variable = $variableName
            CrateName = $crateName
            Version = $currentVersion
        }
    }
    else {
        Write-Host "$crateName$padding`: upgrading $currentVersion -> $newVersion"
        $updates += @{
            Variable = $variableName
            CrateName = $crateName
            OldVersion = $currentVersion
            NewVersion = $newVersion
        }
    }

    # Rate limiting: Wait 1 second before next request
    Start-Sleep -Seconds 1
}

# Apply updates
if ($updates.Count -eq 0) {
    exit 0
}

if ($DryRun) {
    exit 0
}

$updatedContent = $fileContent
foreach ($update in $updates) {
    $oldPattern = "$($update.Variable)=$($update.OldVersion)"
    $newPattern = "$($update.Variable)=$($update.NewVersion)"
    $updatedContent = $updatedContent -replace [regex]::Escape($oldPattern), $newPattern
}

# Write the updated content back to the file
Set-Content -Path $ConstantsFile -Value $updatedContent -NoNewline
