# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Runs the Pester test suite for the release-related PowerShell scripts.

.DESCRIPTION
    Validates that Pester 5.7+ is available, then runs every *.Tests.ps1 file
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

$pester = Get-Module Pester -ListAvailable | Sort-Object Version -Descending | Select-Object -First 1
if ($null -eq $pester -or $pester.Version -lt [version]'5.7.0') {
    Write-Host "ERROR: Pester 5.7+ is required to run the release-script test suite." -ForegroundColor Red
    Write-Host ""
    Write-Host "Install with:"   -ForegroundColor Yellow
    Write-Host "  Install-Module -Name Pester -MinimumVersion 5.7.1 -Force -Scope CurrentUser -SkipPublisherCheck"
    Write-Host ""
    Write-Host "Or run:" -ForegroundColor Yellow
    Write-Host "  just install-tools"
    if ($null -ne $pester) {
        Write-Host ""
        Write-Host "(Detected Pester $($pester.Version); upgrade required.)"
    }
    exit 2
}

Import-Module Pester -MinimumVersion 5.7.0 -Force

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

if ($result.FailedCount -gt 0) {
    exit 1
}
exit 0
