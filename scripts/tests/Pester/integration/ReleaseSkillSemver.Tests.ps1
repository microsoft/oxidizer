# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeDiscovery {
    $null = & cargo semver-checks --version 2>$null
    $script:HasSemverChecks = $LASTEXITCODE -eq 0
}

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    function Invoke-SyntheticSemverCase {
        param(
            [Parameter(Mandatory = $true)][string]$Name,
            [Parameter(Mandatory = $true)][string]$CurrentSource
        )

        $root = Join-Path $TestDrive $Name
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'subject'; Version = '1.0.0' }
            )
        } -Path $root

        $workspace.WriteFile('crates/subject/src/lib.rs', @'
pub fn existing() -> u32 {
    1
}
'@)
        $workspace.AddCommit('feat(subject): establish public API')
        $baseline = $workspace.GitSha()
        $workspace.WriteFile('crates/subject/src/lib.rs', $CurrentSource)

        $oldNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
        $PSNativeCommandUseErrorActionPreference = $false
        try {
            $output = & cargo semver-checks `
                --manifest-path (Join-Path $workspace.Path 'Cargo.toml') `
                --package subject `
                --baseline-rev $baseline `
                --release-type patch `
                --all-features `
                --color never 2>&1 | Out-String
            $exitCode = $LASTEXITCODE
        } finally {
            $PSNativeCommandUseErrorActionPreference = $oldNativeErrorPreference
        }
        return ConvertFrom-SemverChecksOutput `
            -Output $output `
            -ExitCode $exitCode `
            -PackageName subject
    }
}

Describe 'release skill classifications against synthetic Cargo changes' {
    It 'classifies an internal implementation edit as patch' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase -Name patch -CurrentSource @'
pub fn existing() -> u32 {
    helper()
}

fn helper() -> u32 {
    2
}
'@ | Should -Be 'patch'
    }

    It 'leaves backward-compatible additions for source-diff classification' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase -Name additive -CurrentSource @'
pub fn existing() -> u32 {
    1
}

pub fn added() -> u32 {
    2
}
'@ | Should -Be 'patch'
    }

    It 'classifies removal of a public function as breaking' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase -Name breaking -CurrentSource @'
pub fn replacement() -> u32 {
    2
}
'@ | Should -Be 'breaking'
    }
}
