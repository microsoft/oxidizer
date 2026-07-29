# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
}

Describe 'CI SemVer report proc-macro handling' {
    It 'reports explicit manual review without invoking the unsupported proc-macro analysis path' {
        $spec = @{
            Packages = @(
                @{ Name = 'consumer'; Version = '1.0.0'; Deps = @(@{ Name = 'macros' }) }
                @{ Name = 'macros'; Version = '0.2.0'; ProcMacro = $true }
            )
        }
        $workspace = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'semver-proc-macro')
        $workspace.SetVersion('macros', '0.2.1')

        $reportPath = Join-Path $TestDrive 'semver-report.md'
        $outputPath = Join-Path $TestDrive 'github-output.txt'
        $reportScript = Join-Path (Get-OxiRepoRoot) 'scripts\ci\semver-report.ps1'

        & $reportScript `
            -BaseRef HEAD `
            -ReportPath $reportPath `
            -RepoRoot $workspace.Path `
            -GitHubOutput $outputPath 6> $null

        $outputs = Get-Content -Path $outputPath -Raw
        $report = Get-Content -Path $reportPath -Raw

        $outputs | Should -Match '(?m)^publishing=true\r?$'
        $outputs | Should -Match '(?m)^status=warn\r?$'
        $report | Should -Match 'Manual proc-macro SemVer review required'
        $report | Should -Match 'manual proc-macro review required'
        $report | Should -Match 'intentionally does not analyse proc-macro-only targets'
        $report | Should -Not -Match 'no crates with library targets selected'
        $report | Should -Not -Match 'baseline unknown'
        $report | Should -Not -Match 'missing direct-consumer review'
    }

    It 'reports a direct published consumer omitted after a breaking proc-macro release' {
        $spec = @{
            Packages = @(
                @{ Name = 'consumer'; Version = '1.0.0'; Deps = @(@{ Name = 'macros' }) }
                @{ Name = 'macros'; Version = '1.0.0'; ProcMacro = $true }
            )
        }
        $workspace = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'semver-breaking-proc-macro')
        $workspace.SetVersion('macros', '2.0.0')

        $reportPath = Join-Path $TestDrive 'breaking-semver-report.md'
        $outputPath = Join-Path $TestDrive 'breaking-github-output.txt'
        $reportScript = Join-Path (Get-OxiRepoRoot) 'scripts\ci\semver-report.ps1'

        & $reportScript `
            -BaseRef HEAD `
            -ReportPath $reportPath `
            -RepoRoot $workspace.Path `
            -GitHubOutput $outputPath 6> $null

        $outputs = Get-Content -Path $outputPath -Raw
        $report = Get-Content -Path $reportPath -Raw

        $outputs | Should -Match '(?m)^status=warn\r?$'
        $report | Should -Match '<code>consumer</code> — missing direct-consumer review'
        $report | Should -Match 'direct published consumer `consumer` is not in this PR''s publishing set'
        $report | Should -Match 'Re-run the release planner before merging'
    }

    It 'continues CI review propagation through a breaking proc-macro facade' {
        $spec = @{
            Packages = @(
                @{ Name = 'consumer'; Version = '1.0.0'; Deps = @(@{ Name = 'facade_macros' }) }
                @{ Name = 'facade_macros'; Version = '1.0.0'; ProcMacro = $true; Deps = @(@{ Name = 'root_macros' }) }
                @{ Name = 'root_macros'; Version = '1.0.0'; ProcMacro = $true }
            )
        }
        $workspace = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'semver-recursive-proc-macro')
        $workspace.SetVersion('root_macros', '2.0.0')
        $workspace.SetVersion('facade_macros', '2.0.0')

        $reportPath = Join-Path $TestDrive 'recursive-semver-report.md'
        $outputPath = Join-Path $TestDrive 'recursive-github-output.txt'
        $reportScript = Join-Path (Get-OxiRepoRoot) 'scripts\ci\semver-report.ps1'

        & $reportScript `
            -BaseRef HEAD `
            -ReportPath $reportPath `
            -RepoRoot $workspace.Path `
            -GitHubOutput $outputPath 6> $null

        $outputs = Get-Content -Path $outputPath -Raw
        $report = Get-Content -Path $reportPath -Raw

        $outputs | Should -Match '(?m)^status=warn\r?$'
        $report | Should -Match '<code>consumer</code> — missing direct-consumer review'
        $report | Should -Match '`facade_macros` has a breaking release'
        $report | Should -Match '`root_macros`'
        $report | Should -Not -Match 'no crates with library targets selected'
    }

    It 'conservatively continues proc-macro review when the baseline is unknown' {
        $spec = @{
            Packages = @(
                @{ Name = 'consumer'; Version = '1.0.0'; Deps = @(@{ Name = 'macros' }) }
                @{ Name = 'macros'; Version = '1.0.0'; ProcMacro = $true }
            )
        }
        $workspace = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'semver-unknown-proc-macro')

        $reportPath = Join-Path $TestDrive 'unknown-semver-report.md'
        $outputPath = Join-Path $TestDrive 'unknown-github-output.txt'
        $reportScript = Join-Path (Get-OxiRepoRoot) 'scripts\ci\semver-report.ps1'

        & $reportScript `
            -BaseRef refs/heads/missing-semver-test-ref `
            -ReportPath $reportPath `
            -RepoRoot $workspace.Path `
            -GitHubOutput $outputPath 6> $null

        $outputs = Get-Content -Path $outputPath -Raw
        $report = Get-Content -Path $reportPath -Raw

        $outputs | Should -Match '(?m)^status=warn\r?$'
        $report | Should -Match '\| `consumer` .*baseline unknown; manual proc-macro chain review required'
        $report | Should -Match 'CI cannot determine whether its increment is breaking'
        $report | Should -Match 'direct dependency release\(s\) `macros`'
    }
}
