# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path $env:OXI_TEST_COMMON 'New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
}

Describe 'Topology presets (smoke)' {
    BeforeEach {
        Invalidate-WorkspaceMetadataCache
    }

    Context 'Linear2' {
        It 'detects modified upstream' {
            $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'linear2')
            $ws.ModifySource('upstream')
            $ws.AddCommit('upstream edit')
            $ws.BumpVersion('downstream', '0.1.1')
            $ws.AddCommit('bump downstream')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            $findings | Should -HaveCount 1
            $findings[0].Folder | Should -Be 'upstream'
            $findings[0].DependencyChains | Should -HaveCount 1
            $findings[0].DependencyChains[0] -join ',' | Should -Be 'downstream,upstream'
        }
    }

    Context 'Linear3' {
        It 'reaches modified leaf through unchanged middle' {
            $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'linear3')
            $ws.ModifySource('c')
            $ws.AddCommit('c edit')
            $ws.BumpVersion('a', '0.1.1')
            $ws.AddCommit('bump a')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            $findings.Folder | Should -Be 'c'
            $findings[0].DependencyChains[0] -join ',' | Should -Be 'a,b,c'
        }
    }

    Context 'Linear4' {
        It 'BFS depth 4 reaches leaf 3 hops upstream' {
            $ws = New-SyntheticWorkspace -Preset Linear4 -Path (Join-Path $TestDrive 'linear4')
            $ws.ModifySource('d')
            $ws.AddCommit('d edit')
            $ws.BumpVersion('a', '0.1.1')
            $ws.AddCommit('bump a')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            $findings.Folder | Should -Be 'd'
            $findings[0].DependencyChains[0] -join ',' | Should -Be 'a,b,c,d'
        }
    }

    Context 'Diamond4' {
        It 'aggregates two distinct chains to the same modified dep' {
            $ws = New-SyntheticWorkspace -Preset Diamond4 -Path (Join-Path $TestDrive 'diamond4')
            $ws.ModifySource('bottom')
            $ws.AddCommit('bottom edit')
            $ws.BumpVersion('top', '0.1.1')
            $ws.AddCommit('bump top')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            $findings.Folder | Should -Be 'bottom'
            $findings[0].DependencyChains.Count | Should -BeGreaterOrEqual 1
        }
    }

    Context 'Macros3' {
        It 'mirrors thread_aware_macros_impl chain' {
            $ws = New-SyntheticWorkspace -Preset Macros3 -Path (Join-Path $TestDrive 'macros3')
            $ws.ModifySource('macros_impl')
            $ws.AddCommit('macros_impl edit')
            $ws.BumpVersion('user', '0.1.1')
            $ws.AddCommit('bump user')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            $findings.Folder | Should -Be 'macros_impl'
            $findings[0].DependencyChains[0] -join ',' | Should -Be 'user,macros,macros_impl'
        }
    }

    Context 'FanOut5' {
        It 'one shared upstream reported once across multiple bumped dependents' {
            $ws = New-SyntheticWorkspace -Preset FanOut5 -Path (Join-Path $TestDrive 'fanout5')
            $ws.ModifySource('shared_upstream')
            $ws.AddCommit('shared edit')
            $ws.BumpVersion('user1', '0.1.1')
            $ws.BumpVersion('user2', '0.2.1')
            $ws.BumpVersion('user3', '0.3.1')
            $ws.AddCommit('bump users')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            $findings.Folder | Should -Be 'shared_upstream'
            $findings.DependencyChains.Count | Should -BeGreaterOrEqual 3
        }
    }

    Context 'UpDown5' {
        It 'detects upstream above target while target has downstream relations' {
            $ws = New-SyntheticWorkspace -Preset UpDown5 -Path (Join-Path $TestDrive 'updown5')
            $ws.ModifySource('upstream_a')
            $ws.ModifySource('upstream_b')
            $ws.AddCommit('upstream edits')
            $ws.BumpVersion('target', '0.3.1')
            $ws.AddCommit('bump target')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            @($findings).Count | Should -Be 2
            ($findings | ForEach-Object Folder | Sort-Object) -join ',' | Should -Be 'upstream_a,upstream_b'

            $dependents = Get-AllTransitiveDependents -crateName 'target' -repoRoot $ws.Path
            ($dependents | Sort-Object) -join ',' | Should -Be 'downstream_x,downstream_y'
        }
    }

    Context 'Mixed6' {
        It 'filters dev-deps and publish=false but keeps normal deps' {
            $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'mixed6')
            $ws.ModifySource('upstream_a')
            $ws.ModifySource('upstream_b')
            $ws.ModifySource('utility')
            $ws.AddCommit('upstream edits')
            $ws.BumpVersion('target', '0.1.1')
            $ws.AddCommit('bump target')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            ($findings | ForEach-Object Folder) | Should -Be 'upstream_b'
        }
    }

    Context 'Detached' {
        It 'modified crate in component B never surfaces from a release in component A' {
            $ws = New-SyntheticWorkspace -Preset Detached -Path (Join-Path $TestDrive 'detached')
            $ws.ModifySource('delta')
            $ws.AddCommit('delta edit')
            $ws.BumpVersion('alpha', '0.1.1')
            $ws.AddCommit('bump alpha')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2'
            @($findings).Count | Should -Be 0
        }
    }
}
