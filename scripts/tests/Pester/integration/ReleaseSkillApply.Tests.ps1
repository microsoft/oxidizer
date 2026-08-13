# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')

    $script:ApplyPlan = Join-Path (
        Get-OxiRepoRoot
    ) '.github\skills\release-packages\scripts\apply-plan.ps1'

    function Write-TestPlan {
        param(
            [Parameter(Mandatory = $true)][string]$Path,
            [string]$Version = '0.2.0'
        )

        [ordered]@{
            mode = 'targeted'
            releases = @(
                [ordered]@{
                    folder = 'subject'
                    name = 'subject'
                    from = '0.1.0'
                    to = $Version
                    changeType = 'breaking'
                    source = 'user'
                    manualReview = $false
                    cascadeReasons = @()
                }
            )
            warnings = @()
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $Path -Encoding utf8
    }
}

Describe 'apply-plan.ps1' {
    It 'updates only package and workspace dependency version values' {
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'subject'; Version = '0.1.0' }
            )
        } -Path (Join-Path $TestDrive 'apply-success')
        $planPath = Join-Path $workspace.Path 'plan.json'
        Write-TestPlan -Path $planPath

        & $script:ApplyPlan -RepoRoot $workspace.Path -PlanPath $planPath -SkipReadme |
            Out-Null

        Get-Content (Join-Path $workspace.Path 'crates\subject\Cargo.toml') -Raw |
            Should -Match 'version = "0\.2\.0"'
        $root = Get-Content (Join-Path $workspace.Path 'Cargo.toml') -Raw
        $root | Should -Match 'subject = \{ path = "crates/subject", version = "0\.2\.0" \}'
        Test-Path (Join-Path $workspace.Path 'crates\subject\CHANGELOG.md') |
            Should -BeTrue
    }

    It 'restores every written file when validation fails' {
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'subject'; Version = '0.1.0' }
            )
        } -Path (Join-Path $TestDrive 'apply-rollback')
        $workspace.WriteFile('crates/subject/src/lib.rs', 'this is not rust')
        $planPath = Join-Path $workspace.Path 'plan.json'
        Write-TestPlan -Path $planPath
        $rootManifest = Join-Path $workspace.Path 'Cargo.toml'
        $packageManifest = Join-Path $workspace.Path 'crates\subject\Cargo.toml'
        $changelog = Join-Path $workspace.Path 'crates\subject\CHANGELOG.md'
        $rootBefore = [System.IO.File]::ReadAllBytes($rootManifest)
        $packageBefore = [System.IO.File]::ReadAllBytes($packageManifest)
        $changelogBefore = [System.IO.File]::ReadAllBytes($changelog)

        {
            & $script:ApplyPlan -RepoRoot $workspace.Path -PlanPath $planPath -SkipReadme
        } | Should -Throw '*Command failed: cargo check*'

        [System.IO.File]::ReadAllBytes($rootManifest) |
            Should -Be $rootBefore
        [System.IO.File]::ReadAllBytes($packageManifest) |
            Should -Be $packageBefore
        [System.IO.File]::ReadAllBytes($changelog) |
            Should -Be $changelogBefore
    }
}
