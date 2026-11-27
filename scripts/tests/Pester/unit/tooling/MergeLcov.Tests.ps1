# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.7

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $repoRoot = & git rev-parse --show-toplevel
    if ($LASTEXITCODE -ne 0) {
        throw 'Cannot find the repository root.'
    }
    $script:MergeLcov = Join-Path $repoRoot 'scripts/merge-lcov.ps1'
}

Describe 'Merge LCOV reports' {
    It 'keeps a line covered when any feature configuration covers it' {
        $first = Join-Path $TestDrive 'first.info'
        $second = Join-Path $TestDrive 'second.info'
        $output = Join-Path $TestDrive 'merged.info'
        @(
            'TN:'
            'SF:src/lib.rs'
            'DA:1,0'
            'DA:2,4'
            'LF:2'
            'LH:1'
            'end_of_record'
        ) | Set-Content -LiteralPath $first
        @(
            'TN:'
            'SF:src/lib.rs'
            'DA:1,3'
            'DA:2,0'
            'DA:3,1'
            'LF:3'
            'LH:2'
            'end_of_record'
        ) | Set-Content -LiteralPath $second

        & $script:MergeLcov -InputPath @($first, $second) -OutputPath $output

        Get-Content -LiteralPath $output | Should -Be @(
            'TN:'
            'SF:src/lib.rs'
            'DA:1,3'
            'DA:2,4'
            'DA:3,1'
            'LF:3'
            'LH:3'
            'end_of_record'
        )
    }

    It 'retains source records that occur in only one report' {
        $first = Join-Path $TestDrive 'first.info'
        $second = Join-Path $TestDrive 'second.info'
        $output = Join-Path $TestDrive 'merged.info'
        @(
            'TN:'
            'SF:src/first.rs'
            'DA:9,1'
            'LF:1'
            'LH:1'
            'end_of_record'
        ) | Set-Content -LiteralPath $first
        @(
            'TN:'
            'SF:src/second.rs'
            'DA:4,0'
            'LF:1'
            'LH:0'
            'end_of_record'
        ) | Set-Content -LiteralPath $second

        & $script:MergeLcov -InputPath @($first, $second) -OutputPath $output

        $actual = Get-Content -Raw -LiteralPath $output
        $actual | Should -Match 'SF:src/first\.rs'
        $actual | Should -Match 'SF:src/second\.rs'
        $actual | Should -Match 'LH:1'
        $actual | Should -Match 'LH:0'
    }
}
