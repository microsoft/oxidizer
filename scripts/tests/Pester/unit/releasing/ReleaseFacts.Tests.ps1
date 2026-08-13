# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
    Tests for the release skill's deterministic fact-gathering helper. Uses the
    synthetic-workspace
    fixture so the assertions are hermetic (no dependency on the real workspace).
#>

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    $script:FactsScript = Join-Path (
        Get-OxiRepoRoot
    ) '.github\skills\release-packages\scripts\release-facts.ps1'

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
                @{
                    Name = 'exposer'
                    Version = '0.2.0'
                    Deps = @(@{ Name = 'beta' })
                    AllowedExternalTypes = @('beta::*', 'http::*', 'stale::*')
                }
                @{
                    Name = 'gamma_macros'
                    Version = '0.3.0'
                    ProcMacro = $true
                    Deps = @(@{ Name = 'beta' })
                    AllowedExternalTypes = @('gamma_macros::*')
                }
                @{
                    Name = 'devonly'
                    Version = '0.1.0'
                    Deps = @(@{ Name = 'beta'; Kind = 'dev' })
                }
                @{ Name = 'priv_pkg';     Version = '0.4.0'; Published = $false }
            )
        }
        $script:Ws = New-SyntheticWorkspace -Spec $spec -Path $script:WsRoot

        # Create an explicit version-bump commit for 'beta' so it has a real
        # baseline commit (parent 0.2.0 -> commit 0.5.0).
        $script:Ws.SetVersion('beta', '0.5.0')
        $script:Ws.AddCommit('bump beta to 0.5.0')

        # Tag 'beta' as released so everReleased distinguishes it from the
        # never-released crates (whose introducing commit also yields a baseline).
        & git -C $script:Ws.Path tag 'beta-v0.5.0' 2>&1 | Out-Null

        # Leave an uncommitted source edit on 'alpha' so it registers as modified.
        $script:Ws.ModifySource('alpha')

        # Also modify the UNPUBLISHED 'priv_pkg'. This makes the "publish=false is
        # never surfaced" assertion meaningful: priv_pkg now has a real working-tree
        # change, so modified=false can only hold because the published filter
        # suppresses it -- not merely because nothing changed.
        $script:Ws.ModifySource('priv_pkg')

        $script:Facts = Invoke-ReleaseFacts -RepoRoot $script:WsRoot
        $script:ByFolder = @{}
        foreach ($p in $script:Facts.packages) { $script:ByFolder[$p.folder] = $p }
    }

    It 'emits every workspace package under crates/' {
        $folders = @($script:Facts.packages | ForEach-Object { $_.folder }) | Sort-Object
        $folders | Should -Be @('alpha', 'beta', 'devonly', 'exposer', 'gamma_macros', 'priv_pkg')
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
        @($script:ByFolder['devonly'].deps).Count | Should -Be 0
    }

    It 'emits deterministic workspace exposure edges from external-type metadata' {
        $script:ByFolder['exposer'].exposureUnknown | Should -BeFalse
        @($script:ByFolder['exposer'].exposedDeps) | Should -Be @('beta')
    }

    It 'treats missing external-type metadata as no exposure for libraries' {
        $script:ByFolder['alpha'].exposureUnknown | Should -BeFalse
        @($script:ByFolder['alpha'].exposedDeps).Count | Should -Be 0
    }

    It 'includes exposure properties for every package' {
        foreach ($p in $script:Facts.packages) {
            $p.PSObject.Properties.Name | Should -Contain 'exposedDeps'
            $p.PSObject.Properties.Name | Should -Contain 'exposureUnknown'
        }
    }

    It 'flags proc-macro-only packages' {
        $script:ByFolder['gamma_macros'].procMacroOnly | Should -BeTrue
        $script:ByFolder['gamma_macros'].hasLibraryTarget | Should -BeFalse
        $script:ByFolder['beta'].procMacroOnly | Should -BeFalse
    }

    It 'ignores unenforced proc-macro exposure metadata conservatively' {
        $script:ByFolder['gamma_macros'].exposureUnknown | Should -BeTrue
        @($script:ByFolder['gamma_macros'].exposedDeps) | Should -Be @('beta')
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

    It 'distinguishes an ever-released crate from a never-released one via everReleased' {
        # beta is tagged 'beta-v0.5.0'; the others have no release tag. Every crate
        # has a baselineSha (its introducing commit counts as a bump), so
        # everReleased -- not hasBaseline -- is the real discriminator.
        $script:ByFolder['beta'].everReleased  | Should -BeTrue
        $script:ByFolder['alpha'].everReleased | Should -BeFalse
        $script:ByFolder['beta'].hasBaseline   | Should -BeTrue
        $script:ByFolder['alpha'].hasBaseline  | Should -BeTrue
    }

    It 'detects unreleased (working-tree) modifications' {
        $script:ByFolder['alpha'].modified | Should -BeTrue
        $script:ByFolder['alpha'].modifiedFileCount | Should -BeGreaterThan 0
        $script:ByFolder['beta'].modified | Should -BeFalse
    }

    It 'never surfaces publish=false packages as modified' {
        # priv_pkg HAS an uncommitted source edit (see BeforeAll), yet
        # Get-PackagesWithUnreleasedChanges skips it because it is unpublished. If
        # the published filter were removed, this assertion would fail.
        $script:ByFolder['priv_pkg'].modified | Should -BeFalse
    }

    It 'fails loudly for an unresolvable base ref instead of reporting no baseline' {
        { & $script:FactsScript -RepoRoot $script:WsRoot -BaseRef 'refs/heads/no-such-ref-xyz' } |
            Should -Throw '*could not be resolved*'
    }
}
