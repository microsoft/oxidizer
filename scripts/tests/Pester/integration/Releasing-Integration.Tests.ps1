# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Phase 5 — integration tests for the analyses that orchestrate multiple
# helpers. Each test uses a tiny synthetic Cargo workspace and exercises a
# realistic interplay between version changes, source edits, and the
# release-set / unreleased-modified-deps analyses. The N1..N9 scenarios
# previously documented in scripts/tests/RELEASE-DEPS-TEST-CASES.md (since
# deleted) are encoded here.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
}

# --------------------------------------------------------------------------
# Get-UnreleasedModifiedDependencies — BFS / aggregation coverage.
# --------------------------------------------------------------------------

Describe 'Get-UnreleasedModifiedDependencies: BFS / topology' {

    It 'N1 — modified dependency + version-changed dependent in same PR is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n1')
        # Earlier baseline = initial commit. In this PR: modify dependency + change dependent.
        $ws.ModifySource('dependency')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('PR commit')
        # dependent's release artefact must have source modifications past its
        # baseline for the LIVE filter to use it as a BFS root.
        $ws.ModifySource('dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $up = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $up | Should -Not -BeNullOrEmpty
        $up.DependencyChains[0] | Should -Be @('dependent', 'dependency')
        # CurrentVersion threads through from cargo metadata so the menu can
        # render concrete version transitions (e.g. "0.2.0 -> 0.3.0").
        $up.CurrentVersion | Should -Be '0.2.0'
    }

    It 'N2 — earlier-PR dependency edit + current-PR dependent change is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n2')
        # Simulate previous PR landing an dependency edit without a version change:
        $ws.ModifySource('dependency')
        $ws.AddCommit('previous PR: dependency edit')
        # Current PR changes dependent only:
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('current PR: dependent version change')
        $ws.ModifySource('dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'dependency'
    }

    It 'N3 — dependency already version-changed cleanly; no further edits → no finding' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n3')
        # Previous PR: change dependency and release.
        $ws.SetVersion('dependency', '0.2.1')
        $ws.AddCommit('release dependency 0.2.1')
        # Current PR: change dependent only.
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('release dependent 0.1.1')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Count | Should -Be 0
    }

    It 'N4 — change-then-edit dependency is flagged via per-package baseline' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n4')
        # Earlier: change dependency + release.
        $ws.SetVersion('dependency', '0.2.1')
        $ws.AddCommit('release dependency 0.2.1')
        # Later: edit dependency source (no version change).
        $ws.ModifySource('dependency')
        $ws.AddCommit('post-release dependency edit')
        # Current PR: change dependent only.
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('release dependent')
        $ws.ModifySource('dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'dependency'
    }

    It 'N5 — BFS reaches a modified leaf through an unchanged middle' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'n5')
        # Modify the deepest leaf 'c' in an earlier PR.
        $ws.ModifySource('c')
        $ws.AddCommit('previous PR: c edit')
        # Current PR: change 'a' only. Middle 'b' is unchanged.
        $ws.SetVersion('a', '0.1.1')
        $ws.AddCommit('current PR: change a')
        $ws.ModifySource('a')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'c'
        $cFinding = $findings | Where-Object { $_.Folder -eq 'c' }
        $cFinding.DependencyChains[0] | Should -Be @('a', 'b', 'c')
    }

    It 'N6 — CHANGELOG-only edit in dependency still flagged (humans decide materiality)' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n6')
        $changelog = Join-Path $ws.Path 'crates\dependency\CHANGELOG.md'
        Add-Content -Path $changelog -Value "`n* maintenance note`n"
        $ws.AddCommit('previous PR: dependency changelog tweak')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('current PR: change dependent')
        $ws.ModifySource('dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'dependency'
    }

    It 'N7 — publish=false → true flip resets the baseline (pre-flip edits ignored)' {
        Reset-ReleaseScriptCaches
        # Build a workspace where 'dependency' starts as publish=false with pre-flip
        # edits, then is flipped to publish=true on a later commit. Current PR changes
        # dependent only; pre-flip edits must not be reported.
        $spec = @{
            Packages = @(
                @{ Name = 'dependent'; Version = '0.1.0'; Deps = @(@{ Name = 'dependency' }) }
                @{ Name = 'dependency';   Version = '0.2.0'; Published = $false }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'n7')
        # Pre-flip source edit (while publish=false).
        $ws.ModifySource('dependency')
        $ws.AddCommit('pre-flip edit')
        # Flip publish to true.
        $cargo = Join-Path $ws.Path 'crates\dependency\Cargo.toml'
        $content = Get-Content $cargo -Raw
        $content = $content -replace 'publish\s*=\s*false', 'publish = true'
        Set-Content $cargo -Value $content -NoNewline
        $ws.AddCommit('publish=true flip')
        # Current PR: change dependent only.
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('release dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        # No findings: per-package baseline for dependency is the publish-flip commit,
        # newer than the pre-flip edit, so no unreleased changes.
        $findings.Count | Should -Be 0
    }

    It 'N8 — working-tree edits on dependency are flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n8')
        # Current PR: change dependent (committed).
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('change dependent')
        # Uncommitted: tweak dependency source AND dependent source so dependent
        # qualifies as a BFS root under the LIVE filter.
        $ws.ModifySource('dependency')
        $ws.ModifySource('dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'dependency'
    }

    It 'N9 — untracked new file in dependency is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n9')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('change dependent')
        Set-Content -Path (Join-Path $ws.Path 'crates\dependency\src\extra.rs') -Value '// new'
        $ws.ModifySource('dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'dependency'
    }

    It 'T6b — dev-only dep on a modified package is NOT flagged' {
        Reset-ReleaseScriptCaches
        # Mixed6's 'target' has a dev-dep on dependency_a (normal dep on dependency_b).
        $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 't6b')
        $ws.ModifySource('dependency_a')
        $ws.AddCommit('dependency_a edit')
        $ws.SetVersion('target', '0.1.1')
        $ws.AddCommit('change target')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Not -Contain 'dependency_a'
    }

    It 'T15 — publish=false dep is NOT flagged even when modified' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 't15')
        # 'utility' is publish=false. Modify it and version-change dependent_y which depends on it.
        $ws.ModifySource('utility')
        $ws.AddCommit('utility edit')
        $ws.SetVersion('dependent_y', '0.5.1')
        $ws.AddCommit('change dependent_y')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Not -Contain 'utility'
    }

    It 'T16-style aggregation — one shared dependency across multiple version-changed dependents gets multiple chains' {
        Reset-ReleaseScriptCaches
        # Diamond4: top -> {left, right}; left -> bottom; right -> bottom.
        # Modify bottom in an earlier PR; change both left and right.
        $ws = New-SyntheticWorkspace -Preset Diamond4 -Path (Join-Path $TestDrive 't16-style')
        $ws.ModifySource('bottom')
        $ws.AddCommit('previous PR: bottom edit')
        $ws.SetVersion('left',  '0.2.1')
        $ws.SetVersion('right', '0.3.1')
        $ws.AddCommit('current PR: change left + right')
        # Both release-set members must have source mods past their baselines
        # for the LIVE filter to use them as BFS roots.
        $ws.ModifySource('left')
        $ws.ModifySource('right')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $bottom = $findings | Where-Object { $_.Folder -eq 'bottom' }
        $bottom | Should -Not -BeNullOrEmpty
        @($bottom.DependencyChains).Count | Should -Be 2
    }

    It 'Detached — modified package in component B does not surface from a release in component A' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Detached -Path (Join-Path $TestDrive 'detached')
        # Two disconnected components: alpha→beta and gamma→delta.
        # Modify 'gamma' (component B) and change 'alpha' (component A).
        $ws.ModifySource('gamma')
        $ws.AddCommit('mod gamma')
        $ws.SetVersion('alpha', '0.1.1')
        $ws.AddCommit('change alpha')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Not -Contain 'gamma'
        $findings.Folder | Should -Not -Contain 'delta'
    }

    It 'N10 — BFS traverses past release-set intermediates and chains are suffix-subsumed' {
        Reset-ReleaseScriptCaches
        # Linear3: a → b → c. Modify 'c' (unreleased). Release set = {a, b}.
        # Expected: the chain 'a -> b -> c' subsumes 'b -> c', leaving one chain.
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'n10')
        $ws.ModifySource('c')
        $ws.AddCommit('previous PR: c edit')
        $ws.SetVersion('a', '0.1.1')
        $ws.SetVersion('b', '0.2.1')
        $ws.AddCommit('current PR: change a + b')
        # Release-set members must have source mods past their baselines for the
        # LIVE filter to use them as BFS roots.
        $ws.ModifySource('a')
        $ws.ModifySource('b')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'c'
        $cFinding = $findings | Where-Object { $_.Folder -eq 'c' }
        @($cFinding.DependencyChains).Count | Should -Be 1
        @($cFinding.DependencyChains)[0] -join ',' | Should -Be 'a,b,c'
    }

    It 'tags non-release-set findings with InReleaseSet = $false' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'irs-classic')
        $ws.ModifySource('dependency')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('mod dependency + change dependent')
        $ws.ModifySource('dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $u = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeFalse
    }
}

# --------------------------------------------------------------------------
# Get-UnreleasedModifiedDependencies — Invariant B (release-set members
# whose cascade-applied change type is below "breaking" must surface as elevation
# candidates) and the -ModifiedSnapshot mechanism (Invariant A: cascade
# writes must not pollute the working-tree query).
# --------------------------------------------------------------------------

Describe 'Get-UnreleasedModifiedDependencies: release-set elevation (Invariant B)' {

    # Helper: build a Linear2 workspace where 'dependency' is BOTH a release-set
    # member (its version differs from BaseRef) AND has unreleased
    # modifications past its per-package baseline. We arrange this by:
    #   HEAD~2 → initial (dependency at 0.2.0)
    #   HEAD~1 → dependency version-changed to $dependencyPending (this becomes dependency's
    #            per-package baseline; release-set membership against
    #            BaseRef=HEAD~2 depends on the version differing)
    #   HEAD   → source edit on dependency + change dependent so the loop has
    #            something to traverse from. Now dependency is in the release
    #            set AND has modifications post-baseline.
    function script:NewElevationWorkspace {
        param(
            [string]$Path,
            [string]$DependencyPending  # the in-PR pending version for dependency
        )
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path $Path
        $ws.SetVersion('dependency', $DependencyPending)
        $ws.AddCommit('change dependency (pending release)')
        $ws.ModifySource('dependency', '// post-release edit, may warrant elevation')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('mod dependency + change dependent')
        return $ws
    }

    It 'surfaces a release-set member whose cascade-applied change type is patch (0.x — Invariant B)' {
        Reset-ReleaseScriptCaches
        # dependency goes 0.2.0 → 0.2.1 (patch); per Test-IsBreakingChange this
        # is non-breaking, so the user should be prompted to elevate.
        $ws = NewElevationWorkspace -Path (Join-Path $TestDrive 'irs-patch') -DependencyPending '0.2.1'
        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $u = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeTrue
        $u.CurrentVersion | Should -Be '0.2.1'
    }

    It 'does NOT surface a release-set member whose cascade-applied change type is breaking (0.x breaking)' {
        Reset-ReleaseScriptCaches
        # dependency goes 0.2.0 → 0.3.0 (major-on-0.x, i.e. breaking per
        # Test-IsBreakingChange) — no further elevation is possible, so the
        # user should not be prompted.
        $ws = NewElevationWorkspace -Path (Join-Path $TestDrive 'irs-major0x') -DependencyPending '0.3.0'
        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $findings | Where-Object { $_.Folder -eq 'dependency' } | Should -BeNullOrEmpty
    }

    It 'surfaces a release-set member whose cascade-applied change type is non-breaking on 1.x' {
        Reset-ReleaseScriptCaches
        # Build a 1.x workspace so non-breaking (minor) is distinct from
        # breaking (major) in cargo-semver terms.
        $spec = @{
            Packages = @(
                @{ Name = 'dependent'; Version = '1.0.0'; Deps = @(@{ Name = 'dependency' }) }
                @{ Name = 'dependency';   Version = '1.2.3' }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'irs-1x-minor')
        $ws.SetVersion('dependency', '1.3.0')
        $ws.AddCommit('pending minor release of dependency')
        $ws.ModifySource('dependency', '// post-release edit')
        $ws.SetVersion('dependent', '1.0.1')
        $ws.AddCommit('mod dependency + change dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $u = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet   | Should -BeTrue
        $u.CurrentVersion | Should -Be '1.3.0'
    }

    It 'does NOT surface a release-set member whose cascade-applied change type is breaking on 1.x' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'dependent'; Version = '1.0.0'; Deps = @(@{ Name = 'dependency' }) }
                @{ Name = 'dependency';   Version = '1.2.3' }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'irs-1x-major')
        $ws.SetVersion('dependency', '2.0.0')
        $ws.AddCommit('pending major release of dependency')
        $ws.ModifySource('dependency', '// post-release edit')
        $ws.SetVersion('dependent', '1.0.1')
        $ws.AddCommit('mod dependency + change dependent')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $findings | Where-Object { $_.Folder -eq 'dependency' } | Should -BeNullOrEmpty
    }

    It 'still surfaces a release-set member whose pending change type is patch, even when only the working tree carries the modifications' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'irs-worktree')
        $ws.SetVersion('dependency', '0.2.1')
        $ws.AddCommit('pending patch release of dependency')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('change dependent')
        # Uncommitted source edit on dependency — past its per-package baseline.
        $ws.ModifySource('dependency', '// uncommitted further edit')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $u = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeTrue
    }
}

# --------------------------------------------------------------------------
# Get-UnreleasedModifiedDependencies — LIVE-flow filter contract:
#   A release-set member is only treated as a BFS root when it ALSO has its
#   own source modifications past its per-package baseline. Pure-cascade
#   members (version bump only, no source changes) cannot have started
#   consuming new features in their dependencies, so BFS from them would
#   only produce false positives.
# --------------------------------------------------------------------------

Describe 'Get-UnreleasedModifiedDependencies: LIVE-flow BFS-root filter' {

    # Helper: build a Linear2 workspace where 'dependency' has source mods,
    # 'dependent' depends on 'dependency', and provide a ResolvedReleaseSet
    # parameterised by 'dependent's Source ('cascade' or 'user') and
    # whether 'dependent' has its own modifications.
    function script:NewLiveFilterFixture {
        param(
            [string]$Path,
            [switch]$ModifyDependent
        )
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path $Path
        $ws.ModifySource('dependency')
        if ($ModifyDependent) { $ws.ModifySource('dependent') }
        $ws.AddCommit('mods')
        return $ws
    }

    function script:NewSyntheticReleaseSet {
        param([string]$Folder, [string]$Name, [string]$Source)
        @{ $Folder = [pscustomobject]@{
                Folder                  = $Folder
                Name                    = $Name
                CurrentVersion          = '0.1.0'
                EffectiveChangeType     = 'patch'
                EffectiveTargetVersion  = '0.1.1'
                Source                  = $Source
                AutoUpgraded            = $false
                CascadeReasons          = New-Object 'System.Collections.Generic.List[object]'
            } }
    }

    It 'does NOT BFS from a cascade-source release-set member with no own modifications (so its modified deps are not surfaced)' {
        Reset-ReleaseScriptCaches
        # 'dependent' is in the release set but only because of a
        # mechanical cascade bump; it has no source changes of its own.
        # 'dependency' IS modified. Under the LIVE filter, 'dependent' is
        # NOT a BFS root, so 'dependency' is never reached and no findings
        # surface — the user is not pestered about an unreachable dep.
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-cascade-no-mods')
        $set = NewSyntheticReleaseSet -Folder 'dependent' -Name 'dependent' -Source 'cascade'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $findings | Should -BeNullOrEmpty
    }

    It 'DOES BFS from a cascade-source release-set member that has its own modifications (its modified deps surface for review)' {
        Reset-ReleaseScriptCaches
        # Same shape but 'dependent' has its OWN modifications. Under the
        # LIVE filter it IS a BFS root, so 'dependency' is reachable and
        # surfaces as a dep finding. 'dependent' itself also surfaces via
        # the Phase B sweep (Invariant B: cascade-source, below-breaking,
        # with own mods → elevation candidate).
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-cascade-with-mods') -ModifyDependent
        $set = NewSyntheticReleaseSet -Folder 'dependent' -Name 'dependent' -Source 'cascade'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $u = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeFalse
    }

    It 'does NOT BFS from a user-source release-set member without its own modifications (the precondition is mods, regardless of Source)' {
        Reset-ReleaseScriptCaches
        # The LIVE filter is source-agnostic: a user-source release-set
        # member with no source modifications past its baseline is also
        # not a BFS root. (In production this case is rare because the
        # user typically only releases packages they have edited, but the
        # filter is intentionally symmetric — a user-source release-set
        # entry without source mods cannot have started depending on
        # unreleased dependency features either.)
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-user-no-mods')
        $set = NewSyntheticReleaseSet -Folder 'dependent' -Name 'dependent' -Source 'user'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $findings | Should -BeNullOrEmpty
    }

    It 'DOES BFS from a user-source release-set member that has its own modifications' {
        Reset-ReleaseScriptCaches
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-user-with-mods') -ModifyDependent
        $set = NewSyntheticReleaseSet -Folder 'dependent' -Name 'dependent' -Source 'user'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $u = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeFalse
        # User-source members are excluded from Phase B Invariant B sweep,
        # so 'dependent' itself must NOT appear in findings.
        $findings | Where-Object { $_.Folder -eq 'dependent' } | Should -BeNullOrEmpty
    }
}

Describe 'Get-UnreleasedModifiedDependencies: -ModifiedSnapshot honored (Invariant A)' {

    It 'uses the caller-provided snapshot instead of querying the working tree' {
        Reset-ReleaseScriptCaches
        # Build a workspace where the working tree has NO unreleased
        # modifications on dependency — only a pending dependent version change.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'ms-fake-snap')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('pending dependent change')

        # Without a snapshot: the live query finds nothing on dependency.
        $live = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $live.Folder | Should -Not -Contain 'dependency'

        # With a synthetic snapshot claiming both dependency IS modified AND the
        # release-set member 'dependent' has source modifications past its
        # baseline (required by the LIVE filter for dependent to be a BFS
        # root), the BFS surfaces dependency as a classic (non-release-set)
        # finding.
        $snap = @{ 'dependency' = 3; 'dependent' = 1 }
        $with = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1') -ModifiedSnapshot $snap)
        $u = $with | Where-Object { $_.Folder -eq 'dependency' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet     | Should -BeFalse
        $u.ChangedFileCount | Should -Be 3
    }

    It 'returns no findings when the snapshot is empty even if the live query would find some' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'ms-empty-snap')
        # Live: dependency has an unreleased modification past its baseline.
        $ws.ModifySource('dependency')
        $ws.SetVersion('dependent', '0.1.1')
        $ws.AddCommit('mod dependency + change dependent')
        # dependent needs source mods past its baseline to be a BFS root.
        $ws.ModifySource('dependent')

        # Sanity check that the live query DOES find it.
        $live = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $live.Folder | Should -Contain 'dependency'

        # With an empty snapshot, the BFS surfaces nothing.
        $with = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1') -ModifiedSnapshot @{})
        $with.Count | Should -Be 0
    }
}

# --------------------------------------------------------------------------
# Get-UnreleasedModifiedDependencies — -IncludeAllModifiedAsRoots switch.
# Models the "imaginary `*` package depends on every changed package" UX
# without a sentinel: every modified-published package is either reached as
# a dep (real chain recorded) or added as a stub finding with empty chains
# (rendered as "No dependents in release set" by the menu).
# --------------------------------------------------------------------------

Describe 'Get-UnreleasedModifiedDependencies: -IncludeAllModifiedAsRoots' {

    It 'surfaces both changed packages with a real chain when one depends on the other' {
        Reset-ReleaseScriptCaches
        # dependent → dependency. Both modified, no release set yet (iteration 1
        # of all-changed mode). Expect: 2 findings; dependency has chain
        # [dependent, dependency]; dependent has empty chains (no other root
        # reaches it).
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-inter-dep')
        $snap = @{ dependency = 1; dependent = 2 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 2
        $up = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $up | Should -Not -BeNullOrEmpty
        $up.InReleaseSet | Should -BeFalse
        @($up.DependencyChains).Count | Should -Be 1
        $up.DependencyChains[0] | Should -Be @('dependent', 'dependency')

        $dn = $findings | Where-Object { $_.Folder -eq 'dependent' }
        $dn | Should -Not -BeNullOrEmpty
        $dn.InReleaseSet | Should -BeFalse
        @($dn.DependencyChains).Count | Should -Be 0
    }

    It 'surfaces both changed packages as stubs when they have no inter-dependency' {
        Reset-ReleaseScriptCaches
        # Detached preset: alpha → beta, gamma → delta. Modify only beta and
        # delta (the leaves of each disjoint chain). Neither depends on the
        # other, no release set. Expect: 2 stub findings, both with empty
        # chains.
        $ws = New-SyntheticWorkspace -Preset Detached -Path (Join-Path $TestDrive 'iamar-no-inter')
        $snap = @{ beta = 1; delta = 1 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 2
        $findings.Folder | Sort-Object | Should -Be @('beta', 'delta')
        foreach ($f in $findings) {
            $f.InReleaseSet | Should -BeFalse
            @($f.DependencyChains).Count | Should -Be 0
        }
    }

    It 'surfaces a single changed package as a stub when its dependents are unchanged' {
        Reset-ReleaseScriptCaches
        # Linear2: dependent → dependency. Only dependency is changed; dependent
        # is unchanged (and not in release set). With -IncludeAllModifiedAsRoots
        # and empty release set, only dependency is a BFS root. It has no deps
        # of its own, so no chain is recorded — Phase B adds it as a stub.
        # dependent is NOT a finding because it isn't modified.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-lone-changed')
        $snap = @{ dependency = 1 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 1
        $findings[0].Folder | Should -Be 'dependency'
        $findings[0].InReleaseSet | Should -BeFalse
        @($findings[0].DependencyChains).Count | Should -Be 0
    }

    It 'does NOT add stubs for modified-published packages that are user-source release-set members' {
        Reset-ReleaseScriptCaches
        # Linear2: dependent → dependency. Both modified. Release set contains
        # dependent as user-source (the user has already decided to release
        # it). Expect: dependency surfaces as a finding via BFS from dependent;
        # dependent does NOT surface as a stub (user-source members are
        # excluded by the surfacing predicate — the user has already decided).
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-usersrc')
        $rs = @{
            dependent = [pscustomobject]@{
                Folder                = 'dependent'
                Source                = 'user'
                EffectiveChangeType   = 'patch'
                EffectiveTargetVersion = '0.1.1'
                CurrentVersion        = '0.1.0'
                AutoUpgraded          = $false
                CascadeReasons        = @()
            }
        }
        $snap = @{ dependency = 1; dependent = 2 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet $rs `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Folder | Should -Contain 'dependency'
        $findings.Folder | Should -Not -Contain 'dependent'
        $up = $findings | Where-Object { $_.Folder -eq 'dependency' }
        $up.DependencyChains[0] | Should -Be @('dependent', 'dependency')
    }

    It 'returns no findings when both the release set and modified map are empty (regression for early-return)' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-empty')
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot @{} -IncludeAllModifiedAsRoots)
        $findings.Count | Should -Be 0
    }

    It 'behaves identically with or without the switch when no extra changed packages exist beyond the release set' {
        Reset-ReleaseScriptCaches
        # Linear2: only dependency changed; dependent is a user-source release-set
        # member (the user has already decided to release it). Both members carry
        # modifications past their baselines. Without the switch, only dependent
        # is a BFS root and surfaces dependency via 'dependent -> dependency'. With
        # the switch, dependency is also a BFS root (no deps, no extra chains) and
        # Phase B skips it (already a finding). dependent is excluded from the
        # Phase B sweep in both modes because it is user-source. Both calls
        # should produce the same single finding with the same chain.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-regression')
        $rs = @{
            dependent = [pscustomobject]@{
                Folder                = 'dependent'
                Source                = 'user'
                EffectiveChangeType   = 'patch'
                EffectiveTargetVersion = '0.1.1'
                CurrentVersion        = '0.1.0'
                AutoUpgraded          = $false
                CascadeReasons        = @()
            }
        }
        $snap = @{ dependency = 1; dependent = 1 }
        $without = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet $rs -ModifiedSnapshot $snap)
        $with = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet $rs -ModifiedSnapshot $snap `
            -IncludeAllModifiedAsRoots)

        $without.Count | Should -Be 1
        $with.Count    | Should -Be 1
        $without[0].Folder | Should -Be 'dependency'
        $with[0].Folder    | Should -Be 'dependency'
        $without[0].DependencyChains[0] | Should -Be @('dependent', 'dependency')
        $with[0].DependencyChains[0]    | Should -Be @('dependent', 'dependency')
    }
}

# --------------------------------------------------------------------------
# WorkspaceDependencyChains — populated on every finding from
# Get-UnreleasedModifiedDependencies. Records EVERY in-workspace dependency
# chain ending at the finding's folder, irrespective of release-set
# membership. Used by the per-package menu to give the reviewer a
# "big picture" view of what releasing this package could ripple through —
# cascading may pull more dependents into the release set after the prompt,
# so the release-set-rooted DependencyChains would otherwise be misleadingly
# narrow.
# --------------------------------------------------------------------------

Describe 'WorkspaceDependencyChains on findings' {

    It 'records every workspace dependency chain ending at the target (linear)' {
        Reset-ReleaseScriptCaches
        # Linear3: a → b → c. Modify c in an earlier PR; change a only.
        # WorkspaceDependencyChains for c should be the single chain [a,b,c]
        # (the only path in the workspace ending at c).
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'wdc-linear')
        $ws.ModifySource('c')
        $ws.AddCommit('previous PR: c edit')
        $ws.SetVersion('a', '0.1.1')
        $ws.AddCommit('current PR: change a')
        $ws.ModifySource('a')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $cFinding = $findings | Where-Object { $_.Folder -eq 'c' }
        $cFinding | Should -Not -BeNullOrEmpty
        @($cFinding.WorkspaceDependencyChains).Count | Should -Be 1
        $cFinding.WorkspaceDependencyChains[0] | Should -Be @('a', 'b', 'c')
    }

    It 'records both paths in a diamond topology' {
        Reset-ReleaseScriptCaches
        # Diamond4: top → {left, right}; left → bottom; right → bottom.
        # Modify bottom (earlier PR); change top (current PR) so bottom
        # surfaces as a finding. WorkspaceDependencyChains for bottom should
        # contain BOTH paths through the diamond:
        #   top → left → bottom
        #   top → right → bottom
        # — irrespective of which packages are in the release set.
        $ws = New-SyntheticWorkspace -Preset Diamond4 -Path (Join-Path $TestDrive 'wdc-diamond')
        $ws.ModifySource('bottom')
        $ws.AddCommit('previous PR: bottom edit')
        $ws.SetVersion('top', '0.1.1')
        $ws.AddCommit('current PR: change top')
        $ws.ModifySource('top')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $bottom = $findings | Where-Object { $_.Folder -eq 'bottom' }
        $bottom | Should -Not -BeNullOrEmpty
        @($bottom.WorkspaceDependencyChains).Count | Should -Be 2
        $rendered = @($bottom.WorkspaceDependencyChains | ForEach-Object { $_ -join ',' } | Sort-Object)
        $rendered | Should -Be @('top,left,bottom', 'top,right,bottom')
    }

    It 'is empty for a leaf package with no in-workspace dependents (regression for "no in-workspace dependents" menu hint)' {
        Reset-ReleaseScriptCaches
        # Linear2: dependent → dependency. With -IncludeAllModifiedAsRoots the
        # changed dependent surfaces as a stub finding. Nothing else in the
        # workspace depends on dependent, so WorkspaceDependencyChains is @().
        # The menu will render "no in-workspace dependents" for it.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'wdc-leaf')
        $snap = @{ dependent = 1 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 1
        $findings[0].Folder | Should -Be 'dependent'
        @($findings[0].WorkspaceDependencyChains).Count | Should -Be 0
    }

    It 'is independent of release-set membership: same chain whether or not the dependent is in the release set' {
        Reset-ReleaseScriptCaches
        # Linear2: dependent → dependency. Modify dependency in earlier PR; do
        # NOT change dependent (i.e. dependent is NOT in the release set
        # via the BaseRef helper). With -IncludeAllModifiedAsRoots dependency
        # surfaces as a stub (DependencyChains is empty because no release
        # set member depends on it), but WorkspaceDependencyChains must still
        # list [dependent, dependency] — the big-picture view ignores
        # release-set membership.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'wdc-rs-independent')
        $ws.ModifySource('dependency')
        $ws.AddCommit('earlier PR: dependency edit')

        $snap = @{ dependency = 1 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 1
        $up = $findings[0]
        $up.Folder | Should -Be 'dependency'
        # Release-set-rooted DependencyChains is empty (no release-set member
        # depends on dependency — release set is empty here).
        @($up.DependencyChains).Count | Should -Be 0
        # Workspace-wide chains list the full graph path regardless.
        @($up.WorkspaceDependencyChains).Count | Should -Be 1
        $up.WorkspaceDependencyChains[0] | Should -Be @('dependent', 'dependency')
    }
}

# --------------------------------------------------------------------------
# Update-PackageVersion — exercise the [package]-scoped replacement.
# --------------------------------------------------------------------------

Describe 'Update-PackageVersion' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    It 'updates the package version in its own Cargo.toml' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-basic')
        $packageCargo = Join-Path $ws.Path 'crates\dependent\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        $new = Update-PackageVersion -packageName 'dependent' -version '0.1.1' -packageCargoToml $packageCargo -rootCargoToml $rootCargo
        $new | Should -Be '0.1.1'
        (Get-Content $packageCargo -Raw) | Should -Match 'version\s*=\s*"0\.1\.1"'
    }

    It 'updates the [workspace.dependencies] entry for the version-changed package' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-root')
        $packageCargo = Join-Path $ws.Path 'crates\dependency\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'dependency' -version '0.2.1' -packageCargoToml $packageCargo -rootCargoToml $rootCargo | Out-Null
        $rootContent = Get-Content $rootCargo -Raw
        $rootContent | Should -Match 'dependency\s*=\s*\{[^}]*version\s*=\s*"0\.2\.1"'
        # And dependent's version line in the same root table is unchanged.
        $rootContent | Should -Match 'dependent\s*=\s*\{[^}]*version\s*=\s*"0\.1\.0"'
    }

    It 'preserves inline dependency version when the [package] version changes' {
        # Earlier, the package-level regex was `(?<=version\s*=\s*")[^"]+` applied
        # via `-replace`, which clobbers every `version = "..."` in the file —
        # including any inline workspace-dep declarations like
        # `dep = { path = "...", version = "x.y.z" }`. Phase 8 fix scopes the
        # replacement to the [package] table only; this test pins the corrected
        # behavior.
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-inline-dep')

        # Replace the dependent Cargo.toml with one that declares dependency inline
        # (instead of via .workspace = true).
        $dependentCargo = Join-Path $ws.Path 'crates\dependent\Cargo.toml'
        Set-Content -Path $dependentCargo -Value @"
[package]
name = "dependent"
version = "0.1.0"
edition = "2021"
publish = true

[lib]

[dependencies]
dependency = { path = "../dependency", version = "0.2.0" }
"@ -NoNewline

        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'dependent' -version '0.1.1' -packageCargoToml $dependentCargo -rootCargoToml $rootCargo | Out-Null

        $content = Get-Content $dependentCargo -Raw
        # [package] version was updated.
        $content | Should -Match 'name\s*=\s*"dependent"[^\[]*?version\s*=\s*"0\.1\.1"'
        # Inline dependency dep's declared version is preserved.
        if ($content -match 'dependency\s*=\s*\{[^}]*version\s*=\s*"([^"]+)"') {
            $Matches[1] | Should -Be '0.2.0' -Because 'Update-PackageVersion must not rewrite inline workspace-dep versions.'
        } else {
            throw "Could not extract dependency version from rewritten Cargo.toml: $content"
        }
    }

    It 'preserves rust-version when the [package] version changes' {
        # The naive `\bversion` regex was vulnerable to matching `rust-version`
        # because `-` is a non-word character (word boundary lies between `-` and
        # `version`). The shared CargoPackageVersionRegex anchors to line start,
        # so `rust-version = "..."` is no longer confused with the package's
        # version literal. Pin both orderings (rust-version before vs after).
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-rust-version')

        $packageCargo = Join-Path $ws.Path 'crates\dependent\Cargo.toml'
        Set-Content -Path $packageCargo -NoNewline -Value @"
[package]
name = "dependent"
rust-version = "1.88"
version = "0.1.0"
edition = "2021"
publish = true

[lib]
"@

        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'dependent' -version '0.1.1' -packageCargoToml $packageCargo -rootCargoToml $rootCargo | Out-Null

        $content = Get-Content $packageCargo -Raw
        $content | Should -Match 'rust-version\s*=\s*"1\.88"' -Because 'rust-version must be left alone.'
        $content | Should -Match '(?m)^[ \t]*version\s*=\s*"0\.1\.1"'
    }

    It 'does not rewrite the inline version of a sibling crate whose name has the target as a suffix' {
        # The root-Cargo.toml rewrite previously used an un-anchored lookbehind:
        # `(?<=NAME\s*=\s*\{[^\}]*?version\s*=\s*")`. Releasing e.g. `bar` would
        # also match `foo_bar = { ..., version = "..." }` because the regex
        # engine can satisfy the lookbehind by matching `bar` as a suffix of
        # `foo_bar`. The fix anchors the lookbehind to the start of a line
        # under (?m). This test pins the corrected behaviour by constructing
        # an ad-hoc workspace with a deliberately colliding pair.
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'bar';     Version = '0.1.0' }
                @{ Name = 'foo_bar'; Version = '0.2.0' }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'uvc-suffix')

        $packageCargo = Join-Path $ws.Path 'crates\bar\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'bar' -version '0.1.1' -packageCargoToml $packageCargo -rootCargoToml $rootCargo | Out-Null

        $rootContent = Get-Content $rootCargo -Raw
        $rootContent | Should -Match '(?m)^bar\s*=\s*\{[^}]*version\s*=\s*"0\.1\.1"' -Because 'The target crate''s inline version must be updated.'
        $rootContent | Should -Match '(?m)^foo_bar\s*=\s*\{[^}]*version\s*=\s*"0\.2\.0"' -Because 'A sibling crate whose name ends in the target name must not be rewritten.'
    }
}

# --------------------------------------------------------------------------
# Invoke-ResolvedRelease — atomic multi-package on-disk product.
#
# Pins the contract that, when a multi-package plan executes successfully,
# every artefact for every release-set member is written: per-package
# Cargo.toml (new [package] version), workspace root Cargo.toml (new
# inline-dep version in [workspace.dependencies]), per-package CHANGELOG
# (new version section prepended, with cascade-from-dependency bullets on
# cascade-source members), and Update-Readme invoked once per member.
#
# Unit-level coverage of each helper proves the helpers work in isolation;
# this test pins that Invoke-ResolvedRelease wires them all into the per-
# folder loop so a regression that wrote Cargo.toml but skipped CHANGELOG
# (or vice versa) is caught — the regression mode the test-suite review
# identified as the most plausible silent-correctness gap.
# --------------------------------------------------------------------------

Describe 'Invoke-ResolvedRelease: atomic multi-package on-disk product' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    It 'writes Cargo.toml + workspace Cargo.toml + CHANGELOG + Update-Readme call for every plan member' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'invoke-resolved-release-atomic')

        # Replace the bare `## [Unreleased]` placeholder (no trailing newline)
        # with a body that the Extract-UnreleasedSection regex can actually
        # match, so the new version section folds the manually-curated note
        # in — exercising the most common production shape.
        $dependencyChangelog   = Join-Path $ws.Path 'crates\dependency\CHANGELOG.md'
        $dependentChangelog = Join-Path $ws.Path 'crates\dependent\CHANGELOG.md'
        $changelogBody = @(
            '# Changelog',
            '',
            '## [Unreleased]',
            '',
            '- manually curated note',
            ''
        ) -join "`n"
        Set-Content -LiteralPath $dependencyChangelog   -Value $changelogBody -NoNewline
        Set-Content -LiteralPath $dependentChangelog -Value $changelogBody -NoNewline

        # Touch each package with a conventional-commit-formatted message so
        # Write-Changelog has something to fold into the new section — also
        # proves Write-Changelog ran (a no-modification call would early-
        # return with a warning and leave the CHANGELOG untouched).
        $ws.ModifySource('dependency',   '// dependency feature')
        $ws.AddCommit('feat(dependency): add dependency feature')
        $ws.ModifySource('dependent', '// dependent tweak')
        $ws.AddCommit('feat(dependent): use new dependency feature')

        $workspaceBaseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'

        # Hand-build the ResolvedReleaseSet that Resolve-ReleaseSet would
        # produce for: -Packages dependency@non-breaking. On 0.x, non-breaking
        # collapses to a patch bump (0.2.0 -> 0.2.1). Cascade reaches
        # dependent as 'non-breaking' (no cargo_check_external_types declared
        # on either side, so the dep is treated as exposing; the dependency
        # non-breaking is not breaking, so the exposing change type carries
        # through to dependent non-breaking → 0.1.0 -> 0.1.1).
        $dependencyCascadeReasons   = New-Object 'System.Collections.Generic.List[object]'
        $dependentCascadeReasons = New-Object 'System.Collections.Generic.List[object]'
        [void]$dependentCascadeReasons.Add([pscustomobject]@{
            Target   = 'dependency'
            Version  = '0.2.1'
            Breaking = $false
        })

        $resolved = [ordered]@{
            dependency = [pscustomobject]@{
                Folder                   = 'dependency'
                Name                     = 'dependency'
                CurrentVersion           = '0.2.0'
                EffectiveTargetVersion   = '0.2.1'
                EffectiveChangeType      = 'non-breaking'
                Source                   = 'user'
                AutoUpgraded             = $false
                PinHonoredAgainstCascade = $false
                CascadeReasons           = $dependencyCascadeReasons
            }
            dependent = [pscustomobject]@{
                Folder                   = 'dependent'
                Name                     = 'dependent'
                CurrentVersion           = '0.1.0'
                EffectiveTargetVersion   = '0.1.1'
                EffectiveChangeType      = 'non-breaking'
                Source                   = 'cascade'
                AutoUpgraded             = $false
                PinHonoredAgainstCascade = $false
                CascadeReasons           = $dependentCascadeReasons
            }
        }

        # Mock Update-Readme so we can assert it was invoked once per member
        # without depending on cargo-doc2readme being installed or a real
        # README.j2 template existing in the synthetic workspace. The other
        # helpers (Update-PackageVersion / Write-Changelog) are exercised
        # for real so their on-disk side effects are observable.
        Mock -CommandName Update-Readme -MockWith { } -Verifiable:$false

        Push-Location $ws.Path
        try {
            $releases = @(Invoke-ResolvedRelease `
                -RepoRoot $ws.Path `
                -RootCargoToml $rootCargo `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $workspaceBaseline)
        } finally {
            Pop-Location
        }

        # --- Returned records: topo order (deps first), one per release-set member.
        $releases.Count | Should -Be 2
        $releases[0].Package    | Should -Be 'dependency'
        $releases[0].OldVersion | Should -Be '0.2.0'
        $releases[0].NewVersion | Should -Be '0.2.1'
        $releases[1].Package    | Should -Be 'dependent'
        $releases[1].OldVersion | Should -Be '0.1.0'
        $releases[1].NewVersion | Should -Be '0.1.1'

        # --- Per-package Cargo.toml: the [package] version line is rewritten,
        # other [package] fields are preserved verbatim.
        $dependencyCargo   = Get-Content (Join-Path $ws.Path 'crates\dependency\Cargo.toml') -Raw
        $dependentCargo = Get-Content (Join-Path $ws.Path 'crates\dependent\Cargo.toml') -Raw
        $dependencyCargo   | Should -Match '(?m)^version\s*=\s*"0\.2\.1"'
        $dependencyCargo   | Should -Not -Match '(?m)^version\s*=\s*"0\.2\.0"'
        $dependencyCargo   | Should -Match '(?m)^name\s*=\s*"dependency"'
        $dependentCargo | Should -Match '(?m)^version\s*=\s*"0\.1\.1"'
        $dependentCargo | Should -Not -Match '(?m)^version\s*=\s*"0\.1\.0"'
        $dependentCargo | Should -Match '(?m)^name\s*=\s*"dependent"'
        # Dependency declaration in dependent Cargo.toml is preserved (workspace inheritance).
        $dependentCargo | Should -Match '(?m)^dependency\.workspace\s*=\s*true'

        # --- Root Cargo.toml: both [workspace.dependencies] entries are updated.
        $rootContent = Get-Content $rootCargo -Raw
        $rootContent | Should -Match '(?m)^dependency\s*=\s*\{[^}]*version\s*=\s*"0\.2\.1"'
        $rootContent | Should -Match '(?m)^dependent\s*=\s*\{[^}]*version\s*=\s*"0\.1\.1"'
        $rootContent | Should -Not -Match '(?m)^dependency\s*=\s*\{[^}]*version\s*=\s*"0\.2\.0"'
        $rootContent | Should -Not -Match '(?m)^dependent\s*=\s*\{[^}]*version\s*=\s*"0\.1\.0"'

        # --- Per-package CHANGELOG: new version section was prepended.
        $today = (Get-Date).ToString('yyyy-MM-dd')
        $dependencyChangelogText   = Get-Content $dependencyChangelog   -Raw
        $dependentChangelogText = Get-Content $dependentChangelog -Raw

        # Top-level `# Changelog` header is preserved.
        $dependencyChangelogText   | Should -Match '(?m)^# Changelog'
        $dependentChangelogText | Should -Match '(?m)^# Changelog'

        # New version section header (with today's date) appears in both.
        $dependencyChangelogText   | Should -Match ('(?m)^## \[0\.2\.1\] - ' + [regex]::Escape($today))
        $dependentChangelogText | Should -Match ('(?m)^## \[0\.1\.1\] - ' + [regex]::Escape($today))

        # The manually-curated `## [Unreleased]` body line was folded into
        # the new version section (and the now-empty Unreleased heading was
        # stripped — Extract-UnreleasedSection consumed it).
        $dependencyChangelogText   | Should -Match 'manually curated note'
        $dependentChangelogText | Should -Match 'manually curated note'
        $dependencyChangelogText   | Should -Not -Match '(?m)^## \[Unreleased\]'
        $dependentChangelogText | Should -Not -Match '(?m)^## \[Unreleased\]'

        # Conventional-commit bullets from the feat(...) commits are grouped
        # under a `Features` section header.
        $dependencyChangelogText   | Should -Match 'Features'
        $dependencyChangelogText   | Should -Match 'add dependency feature'
        $dependentChangelogText | Should -Match 'Features'
        $dependentChangelogText | Should -Match 'use new dependency feature'

        # dependent is cascade-from-dependency: a Maintenance section with
        # a `Now requires <version> of <target>` bullet must be emitted even
        # though the package only had a feat commit (cascade bullets live in
        # their own section, separate from the conventional-commit ones).
        $dependentChangelogText | Should -Match '🔧 Maintenance'
        $dependentChangelogText | Should -Match 'Now requires `0\.2\.1` of `dependency`'

        # --- Update-Readme: invoked once per release-set member, with the
        # right per-package arguments. This is the README half of the
        # atomicity contract — Update-Readme is the only per-folder side
        # effect that doesn't produce an on-disk artefact in this fixture
        # (no README.j2 template, so the real implementation warns and
        # returns), and asserting the call count + arguments closes the
        # "wrote Cargo.toml but skipped README regen" regression mode.
        Should -Invoke -CommandName Update-Readme -Times 2 -Exactly
        Should -Invoke -CommandName Update-Readme -Times 1 -Exactly `
            -ParameterFilter { $packageName -eq 'dependency' }
        Should -Invoke -CommandName Update-Readme -Times 1 -Exactly `
            -ParameterFilter { $packageName -eq 'dependent' }

        # No README.md was written by the real path either (no template).
        (Test-Path (Join-Path $ws.Path 'crates\dependency\README.md'))   | Should -BeFalse
        (Test-Path (Join-Path $ws.Path 'crates\dependent\README.md')) | Should -BeFalse
    }

    It 'names the DIRECT dependency (not the root cause) in an indirect dependent''s changelog (ADO bug 7536096)' {
        # Linear3: a -> b -> c (a depends on b, b depends on c). Releasing 'c'
        # cascades to BOTH b and a. 'a' depends DIRECTLY on b only — so its
        # changelog must say "Now requires <b's new version> of b", never
        # "of c" (the root cause it does not directly depend on).
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'invoke-resolved-release-indirect')

        $workspaceBaseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'

        # Plan: release c@patch; cascade lifts b and a by patch. CascadeReasons
        # carry the ROOT cause (c) for both b and a — the OLD bug let those leak
        # into a's changelog. The executor must instead derive bullets from each
        # crate's DIRECT deps in the plan.
        $aReasons = New-Object 'System.Collections.Generic.List[object]'
        [void]$aReasons.Add([pscustomobject]@{ Target = 'c'; Version = '0.3.1'; Breaking = $false })
        $bReasons = New-Object 'System.Collections.Generic.List[object]'
        [void]$bReasons.Add([pscustomobject]@{ Target = 'c'; Version = '0.3.1'; Breaking = $false })
        $cReasons = New-Object 'System.Collections.Generic.List[object]'

        $resolved = [ordered]@{
            a = [pscustomobject]@{
                Folder = 'a'; Name = 'a'; CurrentVersion = '0.1.0'; EffectiveTargetVersion = '0.1.1'
                EffectiveChangeType = 'patch'; Source = 'cascade'; AutoUpgraded = $false
                PinHonoredAgainstCascade = $false; CascadeReasons = $aReasons
            }
            b = [pscustomobject]@{
                Folder = 'b'; Name = 'b'; CurrentVersion = '0.2.0'; EffectiveTargetVersion = '0.2.1'
                EffectiveChangeType = 'patch'; Source = 'cascade'; AutoUpgraded = $false
                PinHonoredAgainstCascade = $false; CascadeReasons = $bReasons
            }
            c = [pscustomobject]@{
                Folder = 'c'; Name = 'c'; CurrentVersion = '0.3.0'; EffectiveTargetVersion = '0.3.1'
                EffectiveChangeType = 'patch'; Source = 'user'; AutoUpgraded = $false
                PinHonoredAgainstCascade = $false; CascadeReasons = $cReasons
            }
        }

        Mock -CommandName Update-Readme -MockWith { } -Verifiable:$false

        Push-Location $ws.Path
        try {
            $null = @(Invoke-ResolvedRelease `
                -RepoRoot $ws.Path `
                -RootCargoToml $rootCargo `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $workspaceBaseline)
        } finally {
            Pop-Location
        }

        $aChangelog = Get-Content (Join-Path $ws.Path 'crates\a\CHANGELOG.md') -Raw
        $bChangelog = Get-Content (Join-Path $ws.Path 'crates\b\CHANGELOG.md') -Raw
        $cChangelog = Get-Content (Join-Path $ws.Path 'crates\c\CHANGELOG.md') -Raw

        # The indirect dependent 'a' names its DIRECT dep 'b' at b's NEW version.
        $aChangelog | Should -Match 'Now requires `0\.2\.1` of `b`'
        # It must NOT name the root-cause crate 'c' (the bug being fixed).
        $aChangelog | Should -Not -Match 'of `c`'

        # The direct dependent 'b' names its direct dep 'c'.
        $bChangelog | Should -Match 'Now requires `0\.3\.1` of `c`'

        # The released root 'c' has no direct deps in the set → no cascade bullet.
        $cChangelog | Should -Not -Match 'Now requires'
    }
}

