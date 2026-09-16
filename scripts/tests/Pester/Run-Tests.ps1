# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Runs the Pester test suite for the release-related PowerShell scripts.

.DESCRIPTION
    Validates that Pester 5.7.1 is available, then runs every *.Tests.ps1 file
    under scripts/tests/Pester/. Optionally limits to a single subtree
    (unit|integration|scenarios). Emits NUnit XML if -OutputPath is provided
    (CI consumes this).

.PARAMETER Path
    Optional sub-path under scripts/tests/Pester/ to scope the run (e.g. 'unit',
    'integration', 'scenarios'). Default is to run everything.

.PARAMETER OutputPath
    Optional path for NUnit XML output. Skipped when not provided.

.PARAMETER PassThru
    Return the Pester result object (used by CI to detect failures).
#>
[CmdletBinding()]
param(
    [string]$Path,
    [string]$OutputPath,
    [switch]$PassThru
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

# --- PESTER PRE-FLIGHT ---

$requiredPesterVersion = [version]'5.7.1'
$availablePester = Get-Module Pester -ListAvailable |
    Sort-Object Version -Descending |
    Select-Object -First 1
$pester = Get-Module Pester -ListAvailable |
    Where-Object Version -EQ $requiredPesterVersion |
    Select-Object -First 1
if ($null -eq $pester) {
    Write-Host "ERROR: Pester $requiredPesterVersion is required to run the release-script test suite." -ForegroundColor Red
    Write-Host ""
    Write-Host "Install with:"   -ForegroundColor Yellow
    Write-Host "  Install-Module -Name Pester -RequiredVersion 5.7.1 -Force -Scope CurrentUser -SkipPublisherCheck"
    Write-Host ""
    Write-Host "Then rerun:" -ForegroundColor Yellow
    Write-Host "  just test-scripts"
    if ($null -ne $availablePester) {
        Write-Host ""
        Write-Host "(Detected Pester $($availablePester.Version); version $requiredPesterVersion is required.)"
    }
    exit 2
}

Import-Module Pester -RequiredVersion $requiredPesterVersion -Force

# --- TEST DISCOVERY ---

$testsRoot = Join-Path $PSScriptRoot ''
if ($Path) {
    $testsRoot = Join-Path $PSScriptRoot $Path
    if (-not (Test-Path $testsRoot)) {
        Write-Host "ERROR: No test directory at '$testsRoot'." -ForegroundColor Red
        exit 2
    }
}

# --- RUN ---

$config = New-PesterConfiguration
$config.Run.Path = $testsRoot
$config.Run.PassThru = $true
$config.Output.Verbosity = 'Detailed'
$config.TestResult.Enabled = $false
if ($OutputPath) {
    $config.TestResult.Enabled = $true
    $config.TestResult.OutputFormat = 'NUnitXml'
    $config.TestResult.OutputPath = $OutputPath
}

$result = Invoke-Pester -Configuration $config

if ($PassThru) {
    return $result
}

if ($result.Result -ne 'Passed' -or $result.FailedCount -gt 0 -or $result.FailedContainersCount -gt 0) {
    exit 1
}
exit 0
