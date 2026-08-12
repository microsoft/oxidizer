# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
    Tests for scripts/release-facts.ps1 -- the deterministic fact-gathering
    helper that the AI release skill consumes. Uses the synthetic-workspace
    fixture so the assertions are hermetic (no dependency on the real workspace).
#>

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    $script:FactsScript = Join-Path (Get-OxiRepoRoot) 'scripts\release-facts.ps1'

    function Invoke-ReleaseFacts {
        param([Parameter(Mandatory = $true)][string]$RepoRoot)
        Reset-ReleaseScriptCaches
        $json = & $script:FactsScript -RepoRoot $RepoRoot
        return ($json | ConvertFrom-Json)
    }
}

Describe 'release-facts.ps1' {
    BeforeAll {
        $script:WsRoot = Join-Path $TestDrive 'facts-ws'
        $spec = @{
            Packages = @(
                @{ Name = 'alpha';        Version = '0.1.0'; Deps = @(@{ Name = 'beta' }) }
                @{ Name = 'beta';         Version = '0.2.0' }
                @{ Name = 'gamma_macros'; Version = '0.3.0'; ProcMacro = $true }
                @{ Name = 'priv_pkg';     Version = '0.4.0'; Published = $false }
            )
        }
        $script:Ws = New-SyntheticWorkspace -Spec $spec -Path $script:WsRoot

        # Create an explicit version-bump commit for 'beta' so it has a real
        # baseline commit (parent 0.2.0 -> commit 0.5.0).
        $script:Ws.SetVersion('beta', '0.5.0')
        $script:Ws.AddCommit('bump beta to 0.5.0')

        # Leave an uncommitted source edit on 'alpha' so it registers as modified.
        $script:Ws.ModifySource('alpha')

        $script:Facts = Invoke-ReleaseFacts -RepoRoot $script:WsRoot
        $script:ByFolder = @{}
        foreach ($p in $script:Facts.packages) { $script:ByFolder[$p.folder] = $p }
    }

    It 'emits every workspace package under crates/' {
        $folders = @($script:Facts.packages | ForEach-Object { $_.folder }) | Sort-Object
        $folders | Should -Be @('alpha', 'beta', 'gamma_macros', 'priv_pkg')
    }

    It 'reports name, version and published flag' {
        $script:ByFolder['alpha'].name      | Should -Be 'alpha'
        $script:ByFolder['beta'].version    | Should -Be '0.5.0'
        $script:ByFolder['alpha'].published | Should -BeTrue
        $script:ByFolder['priv_pkg'].published | Should -BeFalse
    }

    It 'captures normal dependency edges (dev excluded)' {
        @($script:ByFolder['alpha'].deps) | Should -Contain 'beta'
        @($script:ByFolder['beta'].deps).Count | Should -Be 0
    }

    It 'flags proc-macro-only packages' {
        $script:ByFolder['gamma_macros'].procMacroOnly | Should -BeTrue
        $script:ByFolder['gamma_macros'].hasLibraryTarget | Should -BeFalse
        $script:ByFolder['beta'].procMacroOnly | Should -BeFalse
    }

    It 'resolves a baseline commit sha for a package with a prior version bump' {
        $script:ByFolder['beta'].hasBaseline | Should -BeTrue
        $script:ByFolder['beta'].baselineSha | Should -Match '^[0-9a-f]{40}$'
    }

    It 'includes a baselineSha property for every package (possibly null)' {
        foreach ($p in $script:Facts.packages) {
            $p.PSObject.Properties.Name | Should -Contain 'baselineSha'
        }
    }

    It 'detects unreleased (working-tree) modifications' {
        $script:ByFolder['alpha'].modified | Should -BeTrue
        $script:ByFolder['alpha'].modifiedFileCount | Should -BeGreaterThan 0
        $script:ByFolder['beta'].modified | Should -BeFalse
    }

    It 'never surfaces publish=false packages as modified' {
        # priv_pkg is unpublished; Get-PackagesWithUnreleasedChanges skips it.
        $script:ByFolder['priv_pkg'].modified | Should -BeFalse
    }
}
