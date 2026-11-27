# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Merges line coverage from multiple LCOV reports.

.DESCRIPTION
    Produces one LCOV report for callers such as Codecov that process one
    feature report at a time. A line is covered when any input report covers it.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string[]]$InputPath,

    [Parameter(Mandatory)]
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
Set-StrictMode -Version Latest

$sources = [System.Collections.Generic.SortedDictionary[string, object]]::new([System.StringComparer]::Ordinal)

foreach ($path in $InputPath) {
    $sourcePath = $null
    $lineCounts = $null

    foreach ($line in [System.IO.File]::ReadLines($path)) {
        if ($line.StartsWith('SF:', [System.StringComparison]::Ordinal)) {
            $sourcePath = $line.Substring(3)
            if (-not $sources.ContainsKey($sourcePath)) {
                $sources[$sourcePath] = [System.Collections.Generic.SortedDictionary[int, long]]::new()
            }
            $lineCounts = $sources[$sourcePath]
            continue
        }

        if ($null -eq $sourcePath -or -not $line.StartsWith('DA:', [System.StringComparison]::Ordinal)) {
            continue
        }

        $parts = $line.Substring(3).Split(',', 3)
        if ($parts.Length -lt 2) {
            throw "Invalid DA record in '$path': $line"
        }

        $lineNumber = [int]$parts[0]
        $hitCount = [long]$parts[1]
        $lineCounts[$lineNumber] = if ($lineCounts.ContainsKey($lineNumber)) {
            [long]::Max($lineCounts[$lineNumber], $hitCount)
        } else {
            $hitCount
        }
    }
}

$content = [System.Collections.Generic.List[string]]::new()
foreach ($sourcePath in $sources.Keys) {
    $lineCounts = $sources[$sourcePath]
    $covered = 0
    $content.Add('TN:')
    $content.Add("SF:$sourcePath")
    foreach ($lineNumber in $lineCounts.Keys) {
        $hitCount = $lineCounts[$lineNumber]
        $content.Add("DA:$lineNumber,$hitCount")
        if ($hitCount -gt 0) {
            $covered++
        }
    }
    $content.Add("LF:$($lineCounts.Count)")
    $content.Add("LH:$covered")
    $content.Add('end_of_record')
}

$outputDirectory = Split-Path -Parent $OutputPath
if (-not $outputDirectory) {
    $outputDirectory = '.'
}
[System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null

$temporaryPath = Join-Path $outputDirectory ".$([System.IO.Path]::GetFileName($OutputPath)).$([System.Guid]::NewGuid().ToString('N')).tmp"
try {
    [System.IO.File]::WriteAllLines(
        $temporaryPath,
        $content,
        [System.Text.UTF8Encoding]::new($false)
    )
    Move-Item -LiteralPath $temporaryPath -Destination $OutputPath -Force
} finally {
    Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
}
