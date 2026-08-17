# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeDiscovery {
    $script:HasSemverChecks =
        $null -ne (Get-Command cargo-semver-checks -ErrorAction SilentlyContinue)
}

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    function Invoke-SyntheticSemverCase {
        param(
            [Parameter(Mandatory = $true)][string]$Name,
            [string]$BaselineSource = @'
pub fn existing() -> u32 {
    1
}
'@,
            [Parameter(Mandatory = $true)][string]$CurrentSource
        )

        $root = Join-Path $TestDrive $Name
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'subject'; Version = '1.0.0' }
            )
        } -Path $root

        $workspace.WriteFile('crates/subject/src/lib.rs', $BaselineSource)
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

    It 'leaves a public parameter type change for source-diff classification' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase `
            -Name parameter-type `
            -BaselineSource @'
pub fn convert(value: u32) -> u32 {
    value
}
'@ `
            -CurrentSource @'
pub fn convert(value: u64) -> u32 {
    value as u32
}
'@ | Should -Be 'patch'
    }

    It 'leaves a public return type change for source-diff classification' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase `
            -Name return-type `
            -BaselineSource @'
pub fn value() -> u32 {
    1
}
'@ `
            -CurrentSource @'
pub fn value() -> u64 {
    1
}
'@ | Should -Be 'patch'
    }

    It 'classifies removal of a public struct field as breaking' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase `
            -Name struct-field `
            -BaselineSource @'
pub struct Config {
    pub value: u32,
}
'@ `
            -CurrentSource @'
pub struct Config {}
'@ | Should -Be 'breaking'
    }

    It 'classifies a required trait method addition as breaking' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase `
            -Name required-trait-method `
            -BaselineSource @'
pub trait Service {
    fn existing(&self);
}
'@ `
            -CurrentSource @'
pub trait Service {
    fn existing(&self);
    fn added(&self);
}
'@ | Should -Be 'breaking'
    }

    It 'classifies an exhaustive enum variant addition as breaking' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase `
            -Name enum-variant `
            -BaselineSource @'
pub enum State {
    Ready,
}
'@ `
            -CurrentSource @'
pub enum State {
    Ready,
    Waiting,
}
'@ | Should -Be 'breaking'
    }

    It 'allows a provided trait method addition' -Skip:(-not $script:HasSemverChecks) {
        Invoke-SyntheticSemverCase `
            -Name provided-trait-method `
            -BaselineSource @'
pub trait Service {
    fn existing(&self);
}
'@ `
            -CurrentSource @'
pub trait Service {
    fn existing(&self);

    fn added(&self) {}
}
'@ | Should -Be 'patch'
    }
}
