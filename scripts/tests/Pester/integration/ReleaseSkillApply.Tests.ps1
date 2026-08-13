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
        $rootManifest = Join-Path $workspace.Path 'Cargo.toml'
        $rootContent = Get-Content -LiteralPath $rootManifest -Raw
        $rootContent.Replace(
            'subject = { path = "crates/subject", version = "0.1.0" }',
            'subject = { path = "crates/subject", default-features = false, version = "0.1.0" }'
        ) | Set-Content -LiteralPath $rootManifest -NoNewline

        & $script:ApplyPlan -RepoRoot $workspace.Path -PlanPath $planPath -SkipReadme |
            Out-Null

        Get-Content (Join-Path $workspace.Path 'crates\subject\Cargo.toml') -Raw |
            Should -Match 'version = "0\.2\.0"'
        $root = Get-Content $rootManifest -Raw
        $root | Should -Match 'subject = \{ path = "crates/subject", default-features = false, version = "0\.2\.0" \}'
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

    It 'runs README generation from RepoRoot and removes newly created files on rollback' {
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'subject'; Version = '0.1.0' }
            )
        } -Path (Join-Path $TestDrive 'apply-readme-rollback')
        $workspace.WriteFile('crates/subject/src/lib.rs', 'this is not rust')
        $readme = Join-Path $workspace.Path 'crates\subject\README.md'
        if (Test-Path -LiteralPath $readme) {
            Remove-Item -LiteralPath $readme -Force
        }
        $planPath = Join-Path $workspace.Path 'plan.json'
        Write-TestPlan -Path $planPath

        $bin = Join-Path $TestDrive 'fake-bin'
        New-Item -ItemType Directory -Path $bin | Out-Null
        if ($IsWindows) {
            @'
@echo off
type nul > crates\subject\README.md
'@ | Set-Content -LiteralPath (Join-Path $bin 'just.cmd') -Encoding ascii
        } else {
            @'
#!/bin/sh
touch crates/subject/README.md
'@ | Set-Content -LiteralPath (Join-Path $bin 'just') -Encoding utf8
            & chmod +x (Join-Path $bin 'just')
        }

        $oldPath = $env:PATH
        $env:PATH = "$bin$([System.IO.Path]::PathSeparator)$oldPath"
        try {
            {
                & $script:ApplyPlan -RepoRoot $workspace.Path -PlanPath $planPath
            } | Should -Throw '*Command failed: cargo check*'
        } finally {
            $env:PATH = $oldPath
        }

        Test-Path -LiteralPath $readme | Should -BeFalse
    }

    It 'rejects a stale plan before leaving version edits behind' {
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'subject'; Version = '0.1.0' }
            )
        } -Path (Join-Path $TestDrive 'apply-stale-plan')
        $planPath = Join-Path $workspace.Path 'plan.json'
        Write-TestPlan -Path $planPath
        $packageManifest = Join-Path $workspace.Path 'crates\subject\Cargo.toml'
        $workspace.SetVersion('subject', '0.1.1')
        $packageBefore = [System.IO.File]::ReadAllBytes($packageManifest)

        {
            & $script:ApplyPlan -RepoRoot $workspace.Path -PlanPath $planPath -SkipReadme
        } | Should -Throw "*not planned version '0.1.0'*"

        [System.IO.File]::ReadAllBytes($packageManifest) |
            Should -Be $packageBefore
    }

    It 'rolls back a package edit when its workspace dependency entry is malformed' {
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'subject'; Version = '0.1.0' }
            )
        } -Path (Join-Path $TestDrive 'apply-malformed-root')
        $planPath = Join-Path $workspace.Path 'plan.json'
        Write-TestPlan -Path $planPath
        $rootManifest = Join-Path $workspace.Path 'Cargo.toml'
        $packageManifest = Join-Path $workspace.Path 'crates\subject\Cargo.toml'
        (Get-Content -LiteralPath $rootManifest -Raw).Replace(
            ', version = "0.1.0"',
            ''
        ) | Set-Content -LiteralPath $rootManifest -NoNewline
        $rootBefore = [System.IO.File]::ReadAllBytes($rootManifest)
        $packageBefore = [System.IO.File]::ReadAllBytes($packageManifest)

        {
            & $script:ApplyPlan -RepoRoot $workspace.Path -PlanPath $planPath -SkipReadme
        } | Should -Throw '*must be one inline table with a version value*'

        [System.IO.File]::ReadAllBytes($rootManifest) | Should -Be $rootBefore
        [System.IO.File]::ReadAllBytes($packageManifest) | Should -Be $packageBefore
    }

    It 'rolls back earlier packages when a later plan entry is stale' {
        $workspace = New-SyntheticWorkspace -Spec @{
            Packages = @(
                @{ Name = 'first'; Version = '0.1.0' }
                @{ Name = 'second'; Version = '0.1.0' }
            )
        } -Path (Join-Path $TestDrive 'apply-multi-rollback')
        $planPath = Join-Path $workspace.Path 'plan.json'
        [ordered]@{
            mode = 'targeted'
            releases = @(
                [ordered]@{
                    folder = 'first'
                    name = 'first'
                    from = '0.1.0'
                    to = '0.2.0'
                    changeType = 'breaking'
                    source = 'user'
                    manualReview = $false
                    cascadeReasons = @()
                }
                [ordered]@{
                    folder = 'second'
                    name = 'second'
                    from = '9.9.9'
                    to = '10.0.0'
                    changeType = 'breaking'
                    source = 'user'
                    manualReview = $false
                    cascadeReasons = @()
                }
            )
            warnings = @()
        } | ConvertTo-Json -Depth 6 |
            Set-Content -LiteralPath $planPath -Encoding utf8

        $rootManifest = Join-Path $workspace.Path 'Cargo.toml'
        $firstManifest = Join-Path $workspace.Path 'crates\first\Cargo.toml'
        $firstChangelog = Join-Path $workspace.Path 'crates\first\CHANGELOG.md'
        $rootBefore = [System.IO.File]::ReadAllBytes($rootManifest)
        $firstBefore = [System.IO.File]::ReadAllBytes($firstManifest)
        $changelogBefore = [System.IO.File]::ReadAllBytes($firstChangelog)

        {
            & $script:ApplyPlan -RepoRoot $workspace.Path -PlanPath $planPath -SkipReadme
        } | Should -Throw "*not planned version '9.9.9'*"

        [System.IO.File]::ReadAllBytes($rootManifest) | Should -Be $rootBefore
        [System.IO.File]::ReadAllBytes($firstManifest) | Should -Be $firstBefore
        [System.IO.File]::ReadAllBytes($firstChangelog) | Should -Be $changelogBefore
    }
}
