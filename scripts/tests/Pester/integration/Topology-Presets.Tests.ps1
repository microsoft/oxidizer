# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
}

Describe 'Topology presets (smoke)' {
    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    Context 'Linear2' {
        It 'detects modified dependency' {
            $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'linear2')
            $ws.ModifySource('dependency')
            $ws.AddCommit('dependency edit')
            $ws.SetVersion('dependent', '0.1.1')
            $ws.AddCommit('change dependent')
            # dependent's release artefact must have source modifications past
            # its baseline for the LIVE filter to use it as a BFS root.
            $ws.ModifySource('dependent')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $up = $findings | Where-Object { $_.Folder -eq 'dependency' }
            $up | Should -Not -BeNullOrEmpty
            $up.DependencyChains | Should -HaveCount 1
            $up.DependencyChains[0] -join ',' | Should -Be 'dependent,dependency'
        }
    }

    Context 'Linear3' {
        It 'reaches modified leaf through unchanged middle' {
            $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'linear3')
            $ws.ModifySource('c')
            $ws.AddCommit('c edit')
            $ws.SetVersion('a', '0.1.1')
            $ws.AddCommit('change a')
            $ws.ModifySource('a')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $cf = $findings | Where-Object { $_.Folder -eq 'c' }
            $cf | Should -Not -BeNullOrEmpty
            $cf.DependencyChains[0] -join ',' | Should -Be 'a,b,c'
        }
    }

    Context 'Linear4' {
        It 'BFS depth 4 reaches leaf 3 hops dependency' {
            $ws = New-SyntheticWorkspace -Preset Linear4 -Path (Join-Path $TestDrive 'linear4')
            $ws.ModifySource('d')
            $ws.AddCommit('d edit')
            $ws.SetVersion('a', '0.1.1')
            $ws.AddCommit('change a')
            $ws.ModifySource('a')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $df = $findings | Where-Object { $_.Folder -eq 'd' }
            $df | Should -Not -BeNullOrEmpty
            $df.DependencyChains[0] -join ',' | Should -Be 'a,b,c,d'
        }
    }

    Context 'Diamond4' {
        It 'aggregates two distinct chains to the same modified dep' {
            $ws = New-SyntheticWorkspace -Preset Diamond4 -Path (Join-Path $TestDrive 'diamond4')
            $ws.ModifySource('bottom')
            $ws.AddCommit('bottom edit')
            $ws.SetVersion('top', '0.1.1')
            $ws.AddCommit('change top')
            $ws.ModifySource('top')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $bf = $findings | Where-Object { $_.Folder -eq 'bottom' }
            $bf | Should -Not -BeNullOrEmpty
            $bf.DependencyChains.Count | Should -BeGreaterOrEqual 1
        }
    }

    Context 'Macros3' {
        It 'mirrors thread_aware_macros_impl chain' {
            $ws = New-SyntheticWorkspace -Preset Macros3 -Path (Join-Path $TestDrive 'macros3')
            $ws.ModifySource('macros_impl')
            $ws.AddCommit('macros_impl edit')
            $ws.SetVersion('user', '0.1.1')
            $ws.AddCommit('change user')
            $ws.ModifySource('user')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $mf = $findings | Where-Object { $_.Folder -eq 'macros_impl' }
            $mf | Should -Not -BeNullOrEmpty
            $mf.DependencyChains[0] -join ',' | Should -Be 'user,macros,macros_impl'
        }
    }

    Context 'FanOut5' {
        It 'one shared dependency reported once across multiple version-changed dependents' {
            $ws = New-SyntheticWorkspace -Preset FanOut5 -Path (Join-Path $TestDrive 'fanout5')
            $ws.ModifySource('shared_dependency')
            $ws.AddCommit('shared edit')
            $ws.SetVersion('user1', '0.1.1')
            $ws.SetVersion('user2', '0.2.1')
            $ws.SetVersion('user3', '0.3.1')
            $ws.AddCommit('change users')
            $ws.ModifySource('user1')
            $ws.ModifySource('user2')
            $ws.ModifySource('user3')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $sh = $findings | Where-Object { $_.Folder -eq 'shared_dependency' }
            $sh | Should -Not -BeNullOrEmpty
            $sh.DependencyChains.Count | Should -BeGreaterOrEqual 3
        }
    }

    Context 'FanInOut5' {
        It 'detects dependency above target while target has dependent relations' {
            $ws = New-SyntheticWorkspace -Preset FanInOut5 -Path (Join-Path $TestDrive 'faninout5')
            $ws.ModifySource('dependency_a')
            $ws.ModifySource('dependency_b')
            $ws.AddCommit('dependency edits')
            $ws.SetVersion('target', '0.3.1')
            $ws.AddCommit('change target')
            $ws.ModifySource('target')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $folders = @($findings | ForEach-Object Folder)
            $folders | Should -Contain 'dependency_a'
            $folders | Should -Contain 'dependency_b'

            $dependents = Get-AllTransitiveDependents -packageName 'target' -repoRoot $ws.Path
            ($dependents | Sort-Object) -join ',' | Should -Be 'dependent_x,dependent_y'
        }
    }

    Context 'Mixed6' {
        It 'filters dev-deps and publish=false but keeps normal deps' {
            $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'mixed6')
            $ws.ModifySource('dependency_a')
            $ws.ModifySource('dependency_b')
            $ws.ModifySource('utility')
            $ws.AddCommit('dependency edits')
            $ws.SetVersion('target', '0.1.1')
            $ws.AddCommit('change target')
            $ws.ModifySource('target')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            $folders = @($findings | ForEach-Object Folder)
            $folders | Should -Contain 'dependency_b'
            $folders | Should -Not -Contain 'dependency_a'  # dev-dep, not surfaced
            $folders | Should -Not -Contain 'utility'     # publish=false
        }
    }

    Context 'Detached' {
        It 'modified package in component B never surfaces from a release in component A' {
            $ws = New-SyntheticWorkspace -Preset Detached -Path (Join-Path $TestDrive 'detached')
            $ws.ModifySource('delta')
            $ws.AddCommit('delta edit')
            $ws.SetVersion('alpha', '0.1.1')
            $ws.AddCommit('change alpha')

            $findings = Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2')
            @($findings).Count | Should -Be 0
        }
    }
}
