#!/usr/bin/env pwsh
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates GitHub Actions versions in workflow files to their latest versions.

.DESCRIPTION
    This script scans all GitHub workflow YAML files, extracts action references,
    queries the GitHub API for the latest version of each action, and updates
    the workflow files with the latest versions. Rate limiting is applied (1 request
    per second) to respect GitHub API usage policies.

.PARAMETER WorkflowsPath
    Path to the .github/workflows directory. Defaults to .github/workflows relative to repo root.

.PARAMETER DryRun
    If specified, shows what would be updated without actually modifying the files.

.PARAMETER GitHubToken
    GitHub personal access token for API requests. If not provided, uses unauthenticated requests
    (lower rate limit). Can also be set via GITHUB_TOKEN environment variable.

.EXAMPLE
    .\update_action_versions.ps1

.EXAMPLE
    .\update_action_versions.ps1 -DryRun

.EXAMPLE
    .\update_action_versions.ps1 -GitHubToken "ghp_..."
#>

param(
    [string]$WorkflowsPath = (Join-Path $PSScriptRoot ".." ".github" "workflows"),
    [switch]$DryRun,
    [string]$GitHubToken = $env:GITHUB_TOKEN
)

$ErrorActionPreference = "Stop"

function Get-ActionLatestVersion {
    param(
        [string]$Owner,
        [string]$Repo
    )

    try {
        $headers = @{
            "User-Agent" = "oxidizer-action-updater (github.com/microsoft/oxidizer)"
            "Accept" = "application/vnd.github+json"
        }

        if ($GitHubToken) {
            $headers["Authorization"] = "Bearer $GitHubToken"
        }

        # Try to get the latest release first
        $releaseUrl = "https://api.github.com/repos/$Owner/$Repo/releases/latest"

        try {
            $response = Invoke-RestMethod -Uri $releaseUrl -Headers $headers -ErrorAction Stop
            $latestVersion = $response.tag_name
            return @{
                Success = $true
                Version = $latestVersion
            }
        }
        catch {
            # If no releases, try to get the latest tag
            $tagsUrl = "https://api.github.com/repos/$Owner/$Repo/tags"

            $tagsResponse = Invoke-RestMethod -Uri $tagsUrl -Headers $headers -ErrorAction Stop

            if ($tagsResponse.Count -eq 0) {
                return @{
                    Success = $false
                    Error = "No tags found"
                }
            }

            $latestTag = $tagsResponse[0].name
            return @{
                Success = $true
                Version = $latestTag
            }
        }
    }
    catch {
        $statusCode = $_.Exception.Response.StatusCode.value__
        $errorMsg = if ($statusCode -eq 404) {
            "Not found (404)"
        }
        elseif ($statusCode -eq 403) {
            "Rate limit exceeded or forbidden (403)"
        }
        else {
            $_.Exception.Message
        }

        return @{
            Success = $false
            Error = $errorMsg
        }
    }
}

function Parse-ActionReference {
    param(
        [string]$ActionRef
    )

    # Parse action references in the format:
    # - owner/repo@version
    # - owner/repo/path@version

    if ($ActionRef -match '^([^/]+)/([^/@]+)(?:/[^@]+)?@(.+)$') {
        return @{
            Owner = $Matches[1]
            Repo = $Matches[2]
            Version = $Matches[3]
            FullRef = $ActionRef
        }
    }

    return $null
}

function Get-ActionReferenceWithNewVersion {
    param(
        [string]$OriginalRef,
        [string]$NewVersion
    )

    # Replace the version part while preserving the rest
    if ($OriginalRef -match '^(.+)@(.+)$') {
        return "$($Matches[1])@$NewVersion"
    }

    return $OriginalRef
}

function Test-VersionIsSHA {
    param(
        [string]$Version
    )

    # Check if version is a SHA (40 hex characters)
    return $Version -match '^[0-9a-f]{40}$'
}

function Test-VersionIsBranch {
    param(
        [string]$Version
    )

    # Common branch names to skip updating
    $branchNames = @('main', 'master', 'develop', 'dev')
    return $branchNames -contains $Version
}

function Compare-Versions {
    param(
        [string]$Current,
        [string]$Latest
    )

    # Simple comparison - returns $true if versions are different
    return $Current -ne $Latest
}

# Main script execution
if (-not (Test-Path $WorkflowsPath)) {
    Write-Host "Error: Workflows directory not found at '$WorkflowsPath'"
    exit 1
}

# Find all workflow files
$workflowFiles = Get-ChildItem -Path $WorkflowsPath -Filter "*.yml" -File
$workflowFiles += Get-ChildItem -Path $WorkflowsPath -Filter "*.yaml" -File -ErrorAction SilentlyContinue

if ($workflowFiles.Count -eq 0) {
    exit 0
}

# Track all actions and their occurrences
$actionReferences = @{}
$fileUpdates = @{}

# Scan all workflow files for action references
foreach ($file in $workflowFiles) {
    $content = Get-Content $file.FullName -Raw
    $lines = Get-Content $file.FullName

    $lineNumber = 0
    foreach ($line in $lines) {
        $lineNumber++

        # Match 'uses:' lines (with proper YAML indentation)
        if ($line -match '^\s+uses:\s+(.+?)\s*$') {
            $actionRef = $Matches[1].Trim()

            # Skip local actions (start with ./)
            if ($actionRef -like "./*") {
                continue
            }

            $parsed = Parse-ActionReference -ActionRef $actionRef

            if ($null -eq $parsed) {
                continue
            }

            $actionKey = "$($parsed.Owner)/$($parsed.Repo)"

            if (-not $actionReferences.ContainsKey($actionKey)) {
                $actionReferences[$actionKey] = @{
                    Owner = $parsed.Owner
                    Repo = $parsed.Repo
                    Occurrences = @()
                }
            }

            $actionReferences[$actionKey].Occurrences += @{
                File = $file.FullName
                FileName = $file.Name
                LineNumber = $lineNumber
                OriginalRef = $actionRef
                CurrentVersion = $parsed.Version
            }
        }
    }
}

if ($actionReferences.Count -eq 0) {
    exit 0
}

# Calculate maximum action name length for padding
$maxLength = ($actionReferences.Keys | ForEach-Object { $_.Length } | Measure-Object -Maximum).Maximum

# Query GitHub API for latest versions
$updates = @()
$skipped = @()
$unchanged = @()

foreach ($actionKey in $actionReferences.Keys) {
    $action = $actionReferences[$actionKey]
    $padding = " " * ($maxLength - $actionKey.Length)

    # Check if version is a SHA or branch name
    $firstVersion = $action.Occurrences[0].CurrentVersion
    if (Test-VersionIsSHA -Version $firstVersion) {
        Write-Host "$actionKey$padding`: skipped, using commit SHA"
        Start-Sleep -Seconds 1
        continue
    }

    if (Test-VersionIsBranch -Version $firstVersion) {
        Write-Host "$actionKey$padding`: skipped, using branch name"
        Start-Sleep -Seconds 1
        continue
    }

    # Query for latest version
    $result = Get-ActionLatestVersion -Owner $action.Owner -Repo $action.Repo

    if (-not $result.Success) {
        Write-Host "$actionKey$padding`: $($result.Error)"
        $skipped += @{
            ActionKey = $actionKey
            Reason = $result.Error
        }
        Start-Sleep -Seconds 1
        continue
    }

    $latestVersion = $result.Version

    # Check if update is needed
    $needsUpdate = $false
    foreach ($occurrence in $action.Occurrences) {
        if (Compare-Versions -Current $occurrence.CurrentVersion -Latest $latestVersion) {
            $needsUpdate = $true
            break
        }
    }

    if (-not $needsUpdate) {
        Write-Host "$actionKey$padding`: unchanged, $firstVersion is the latest version"
        $unchanged += @{
            ActionKey = $actionKey
            Version = $latestVersion
        }
    }
    else {
        # Collect all unique version transitions
        $versions = $action.Occurrences | Select-Object -ExpandProperty CurrentVersion -Unique
        if ($versions.Count -eq 1) {
            Write-Host "$actionKey$padding`: upgrading $firstVersion -> $latestVersion"
        }
        else {
            Write-Host "$actionKey$padding`: upgrading $($versions -join ', ') -> $latestVersion"
        }

        foreach ($occurrence in $action.Occurrences) {
            if (Compare-Versions -Current $occurrence.CurrentVersion -Latest $latestVersion) {
                $newRef = Get-ActionReferenceWithNewVersion -OriginalRef $occurrence.OriginalRef -NewVersion $latestVersion

                $updates += @{
                    ActionKey = $actionKey
                    File = $occurrence.File
                    FileName = $occurrence.FileName
                    LineNumber = $occurrence.LineNumber
                    OldRef = $occurrence.OriginalRef
                    NewRef = $newRef
                    OldVersion = $occurrence.CurrentVersion
                    NewVersion = $latestVersion
                }
            }
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

# Group updates by file
$updatesByFile = $updates | Group-Object -Property File
foreach ($fileGroup in $updatesByFile) {
    $filePath = $fileGroup.Name

    $content = Get-Content $filePath -Raw

    foreach ($update in $fileGroup.Group) {
        # Use regex to match the exact line with proper escaping
        $oldPattern = [regex]::Escape("uses: $($update.OldRef)")
        $newPattern = "uses: $($update.NewRef)"

        if ($content -match $oldPattern) {
            $content = $content -replace $oldPattern, $newPattern
        }
    }

    # Write the updated content back to the file
    Set-Content -Path $filePath -Value $content -NoNewline
}
