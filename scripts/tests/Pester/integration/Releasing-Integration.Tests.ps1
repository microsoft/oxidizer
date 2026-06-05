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
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
    . (Join-Path $env:OXI_TEST_COMMON 'New-SyntheticWorkspace.ps1')
}

# --------------------------------------------------------------------------
# Get-UnreleasedModifiedDependencies — BFS / aggregation coverage.
# --------------------------------------------------------------------------

Describe 'Get-UnreleasedModifiedDependencies: BFS / topology' {

    It 'N1 — modified upstream + version-changed downstream in same PR is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n1')
        # Earlier baseline = initial commit. In this PR: modify upstream + change downstream.
        $ws.ModifySource('upstream')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('PR commit')
        # downstream's release artefact must have source modifications past its
        # baseline for the LIVE filter to use it as a BFS root.
        $ws.ModifySource('downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $up = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $up | Should -Not -BeNullOrEmpty
        $up.DependencyChains[0] | Should -Be @('downstream', 'upstream')
        # CurrentVersion threads through from cargo metadata so the menu can
        # render concrete version transitions (e.g. "0.2.0 -> 0.3.0").
        $up.CurrentVersion | Should -Be '0.2.0'
    }

    It 'N2 — earlier-PR upstream edit + current-PR downstream change is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n2')
        # Simulate previous PR landing an upstream edit without a version change:
        $ws.ModifySource('upstream')
        $ws.AddCommit('previous PR: upstream edit')
        # Current PR changes downstream only:
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('current PR: downstream version change')
        $ws.ModifySource('downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'N3 — upstream already version-changed cleanly; no further edits → no finding' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n3')
        # Previous PR: change upstream and release.
        $ws.SetVersion('upstream', '0.2.1')
        $ws.AddCommit('release upstream 0.2.1')
        # Current PR: change downstream only.
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('release downstream 0.1.1')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Count | Should -Be 0
    }

    It 'N4 — change-then-edit upstream is flagged via per-package baseline' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n4')
        # Earlier: change upstream + release.
        $ws.SetVersion('upstream', '0.2.1')
        $ws.AddCommit('release upstream 0.2.1')
        # Later: edit upstream source (no version change).
        $ws.ModifySource('upstream')
        $ws.AddCommit('post-release upstream edit')
        # Current PR: change downstream only.
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('release downstream')
        $ws.ModifySource('downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'upstream'
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

    It 'N6 — CHANGELOG-only edit in upstream still flagged (humans decide materiality)' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n6')
        $changelog = Join-Path $ws.Path 'crates\upstream\CHANGELOG.md'
        Add-Content -Path $changelog -Value "`n* maintenance note`n"
        $ws.AddCommit('previous PR: upstream changelog tweak')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('current PR: change downstream')
        $ws.ModifySource('downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'N7 — publish=false → true flip resets the baseline (pre-flip edits ignored)' {
        Reset-ReleaseScriptCaches
        # Build a workspace where 'upstream' starts as publish=false with pre-flip
        # edits, then is flipped to publish=true on a later commit. Current PR changes
        # downstream only; pre-flip edits must not be reported.
        $spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '0.1.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '0.2.0'; Published = $false }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'n7')
        # Pre-flip source edit (while publish=false).
        $ws.ModifySource('upstream')
        $ws.AddCommit('pre-flip edit')
        # Flip publish to true.
        $cargo = Join-Path $ws.Path 'crates\upstream\Cargo.toml'
        $content = Get-Content $cargo -Raw
        $content = $content -replace 'publish\s*=\s*false', 'publish = true'
        Set-Content $cargo -Value $content -NoNewline
        $ws.AddCommit('publish=true flip')
        # Current PR: change downstream only.
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('release downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        # No findings: per-package baseline for upstream is the publish-flip commit,
        # newer than the pre-flip edit, so no unreleased changes.
        $findings.Count | Should -Be 0
    }

    It 'N8 — working-tree edits on upstream are flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n8')
        # Current PR: change downstream (committed).
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('change downstream')
        # Uncommitted: tweak upstream source AND downstream source so downstream
        # qualifies as a BFS root under the LIVE filter.
        $ws.ModifySource('upstream')
        $ws.ModifySource('downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'N9 — untracked new file in upstream is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n9')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('change downstream')
        Set-Content -Path (Join-Path $ws.Path 'crates\upstream\src\extra.rs') -Value '// new'
        $ws.ModifySource('downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'T6b — dev-only dep on a modified package is NOT flagged' {
        Reset-ReleaseScriptCaches
        # Mixed6's 'target' has a dev-dep on upstream_a (normal dep on upstream_b).
        $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 't6b')
        $ws.ModifySource('upstream_a')
        $ws.AddCommit('upstream_a edit')
        $ws.SetVersion('target', '0.1.1')
        $ws.AddCommit('change target')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Not -Contain 'upstream_a'
    }

    It 'T15 — publish=false dep is NOT flagged even when modified' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 't15')
        # 'utility' is publish=false. Modify it and version-change downstream_y which depends on it.
        $ws.ModifySource('utility')
        $ws.AddCommit('utility edit')
        $ws.SetVersion('downstream_y', '0.5.1')
        $ws.AddCommit('change downstream_y')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $findings.Folder | Should -Not -Contain 'utility'
    }

    It 'T16-style aggregation — one shared upstream across multiple version-changed downstreams gets multiple chains' {
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
        $ws.ModifySource('upstream')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('mod upstream + change downstream')
        $ws.ModifySource('downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
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

    # Helper: build a Linear2 workspace where 'upstream' is BOTH a release-set
    # member (its version differs from BaseRef) AND has unreleased
    # modifications past its per-package baseline. We arrange this by:
    #   HEAD~2 → initial (upstream at 0.2.0)
    #   HEAD~1 → upstream version-changed to $upstreamPending (this becomes upstream's
    #            per-package baseline; release-set membership against
    #            BaseRef=HEAD~2 depends on the version differing)
    #   HEAD   → source edit on upstream + change downstream so the loop has
    #            something to traverse from. Now upstream is in the release
    #            set AND has modifications post-baseline.
    function script:NewElevationWorkspace {
        param(
            [string]$Path,
            [string]$UpstreamPending  # the in-PR pending version for upstream
        )
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path $Path
        $ws.SetVersion('upstream', $UpstreamPending)
        $ws.AddCommit('change upstream (pending release)')
        $ws.ModifySource('upstream', '// post-release edit, may warrant elevation')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('mod upstream + change downstream')
        return $ws
    }

    It 'surfaces a release-set member whose cascade-applied change type is patch (0.x — Invariant B)' {
        Reset-ReleaseScriptCaches
        # upstream goes 0.2.0 → 0.2.1 (patch); per Test-IsBreakingChange this
        # is non-breaking, so the user should be prompted to elevate.
        $ws = NewElevationWorkspace -Path (Join-Path $TestDrive 'irs-patch') -UpstreamPending '0.2.1'
        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeTrue
        $u.CurrentVersion | Should -Be '0.2.1'
    }

    It 'does NOT surface a release-set member whose cascade-applied change type is breaking (0.x breaking)' {
        Reset-ReleaseScriptCaches
        # upstream goes 0.2.0 → 0.3.0 (major-on-0.x, i.e. breaking per
        # Test-IsBreakingChange) — no further elevation is possible, so the
        # user should not be prompted.
        $ws = NewElevationWorkspace -Path (Join-Path $TestDrive 'irs-major0x') -UpstreamPending '0.3.0'
        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $findings | Where-Object { $_.Folder -eq 'upstream' } | Should -BeNullOrEmpty
    }

    It 'surfaces a release-set member whose cascade-applied change type is non-breaking on 1.x' {
        Reset-ReleaseScriptCaches
        # Build a 1.x workspace so non-breaking (minor) is distinct from
        # breaking (major) in cargo-semver terms.
        $spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'irs-1x-minor')
        $ws.SetVersion('upstream', '1.3.0')
        $ws.AddCommit('pending minor release of upstream')
        $ws.ModifySource('upstream', '// post-release edit')
        $ws.SetVersion('downstream', '1.0.1')
        $ws.AddCommit('mod upstream + change downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet   | Should -BeTrue
        $u.CurrentVersion | Should -Be '1.3.0'
    }

    It 'does NOT surface a release-set member whose cascade-applied change type is breaking on 1.x' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'irs-1x-major')
        $ws.SetVersion('upstream', '2.0.0')
        $ws.AddCommit('pending major release of upstream')
        $ws.ModifySource('upstream', '// post-release edit')
        $ws.SetVersion('downstream', '1.0.1')
        $ws.AddCommit('mod upstream + change downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $findings | Where-Object { $_.Folder -eq 'upstream' } | Should -BeNullOrEmpty
    }

    It 'still surfaces a release-set member whose pending change type is patch, even when only the working tree carries the modifications' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'irs-worktree')
        $ws.SetVersion('upstream', '0.2.1')
        $ws.AddCommit('pending patch release of upstream')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('change downstream')
        # Uncommitted source edit on upstream — past its per-package baseline.
        $ws.ModifySource('upstream', '// uncommitted further edit')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~2'))
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
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

    # Helper: build a Linear2 workspace where 'upstream' has source mods,
    # 'downstream' depends on 'upstream', and provide a ResolvedReleaseSet
    # parameterised by 'downstream's Source ('cascade' or 'user') and
    # whether 'downstream' has its own modifications.
    function script:NewLiveFilterFixture {
        param(
            [string]$Path,
            [switch]$ModifyDownstream
        )
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path $Path
        $ws.ModifySource('upstream')
        if ($ModifyDownstream) { $ws.ModifySource('downstream') }
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
        # 'downstream' is in the release set but only because of a
        # mechanical cascade bump; it has no source changes of its own.
        # 'upstream' IS modified. Under the LIVE filter, 'downstream' is
        # NOT a BFS root, so 'upstream' is never reached and no findings
        # surface — the user is not pestered about an unreachable dep.
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-cascade-no-mods')
        $set = NewSyntheticReleaseSet -Folder 'downstream' -Name 'downstream' -Source 'cascade'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $findings | Should -BeNullOrEmpty
    }

    It 'DOES BFS from a cascade-source release-set member that has its own modifications (its modified deps surface for review)' {
        Reset-ReleaseScriptCaches
        # Same shape but 'downstream' has its OWN modifications. Under the
        # LIVE filter it IS a BFS root, so 'upstream' is reachable and
        # surfaces as a dep finding. 'downstream' itself also surfaces via
        # the Phase B sweep (Invariant B: cascade-source, below-breaking,
        # with own mods → elevation candidate).
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-cascade-with-mods') -ModifyDownstream
        $set = NewSyntheticReleaseSet -Folder 'downstream' -Name 'downstream' -Source 'cascade'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
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
        # unreleased upstream features either.)
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-user-no-mods')
        $set = NewSyntheticReleaseSet -Folder 'downstream' -Name 'downstream' -Source 'user'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $findings | Should -BeNullOrEmpty
    }

    It 'DOES BFS from a user-source release-set member that has its own modifications' {
        Reset-ReleaseScriptCaches
        $ws = NewLiveFilterFixture -Path (Join-Path $TestDrive 'live-user-with-mods') -ModifyDownstream
        $set = NewSyntheticReleaseSet -Folder 'downstream' -Name 'downstream' -Source 'user'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet $set)
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeFalse
        # User-source members are excluded from Phase B Invariant B sweep,
        # so 'downstream' itself must NOT appear in findings.
        $findings | Where-Object { $_.Folder -eq 'downstream' } | Should -BeNullOrEmpty
    }
}

Describe 'Get-UnreleasedModifiedDependencies: -ModifiedSnapshot honored (Invariant A)' {

    It 'uses the caller-provided snapshot instead of querying the working tree' {
        Reset-ReleaseScriptCaches
        # Build a workspace where the working tree has NO unreleased
        # modifications on upstream — only a pending downstream version change.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'ms-fake-snap')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('pending downstream change')

        # Without a snapshot: the live query finds nothing on upstream.
        $live = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $live.Folder | Should -Not -Contain 'upstream'

        # With a synthetic snapshot claiming both upstream IS modified AND the
        # release-set member 'downstream' has source modifications past its
        # baseline (required by the LIVE filter for downstream to be a BFS
        # root), the BFS surfaces upstream as a classic (non-release-set)
        # finding.
        $snap = @{ 'upstream' = 3; 'downstream' = 1 }
        $with = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1') -ModifiedSnapshot $snap)
        $u = $with | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet     | Should -BeFalse
        $u.ChangedFileCount | Should -Be 3
    }

    It 'returns no findings when the snapshot is empty even if the live query would find some' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'ms-empty-snap')
        # Live: upstream has an unreleased modification past its baseline.
        $ws.ModifySource('upstream')
        $ws.SetVersion('downstream', '0.1.1')
        $ws.AddCommit('mod upstream + change downstream')
        # downstream needs source mods past its baseline to be a BFS root.
        $ws.ModifySource('downstream')

        # Sanity check that the live query DOES find it.
        $live = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -ResolvedReleaseSet (New-ResolvedReleaseSetFromBaseRef -RepoRoot $ws.Path -BaseRef 'HEAD~1'))
        $live.Folder | Should -Contain 'upstream'

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
        # downstream → upstream. Both modified, no release set yet (iteration 1
        # of all-changed mode). Expect: 2 findings; upstream has chain
        # [downstream, upstream]; downstream has empty chains (no other root
        # reaches it).
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-inter-dep')
        $snap = @{ upstream = 1; downstream = 2 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 2
        $up = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $up | Should -Not -BeNullOrEmpty
        $up.InReleaseSet | Should -BeFalse
        @($up.DependencyChains).Count | Should -Be 1
        $up.DependencyChains[0] | Should -Be @('downstream', 'upstream')

        $dn = $findings | Where-Object { $_.Folder -eq 'downstream' }
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
        # Linear2: downstream → upstream. Only upstream is changed; downstream
        # is unchanged (and not in release set). With -IncludeAllModifiedAsRoots
        # and empty release set, only upstream is a BFS root. It has no deps
        # of its own, so no chain is recorded — Phase B adds it as a stub.
        # downstream is NOT a finding because it isn't modified.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-lone-changed')
        $snap = @{ upstream = 1 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 1
        $findings[0].Folder | Should -Be 'upstream'
        $findings[0].InReleaseSet | Should -BeFalse
        @($findings[0].DependencyChains).Count | Should -Be 0
    }

    It 'does NOT add stubs for modified-published packages that are user-source release-set members' {
        Reset-ReleaseScriptCaches
        # Linear2: downstream → upstream. Both modified. Release set contains
        # downstream as user-source (the user has already decided to release
        # it). Expect: upstream surfaces as a finding via BFS from downstream;
        # downstream does NOT surface as a stub (user-source members are
        # excluded by the surfacing predicate — the user has already decided).
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-usersrc')
        $rs = @{
            downstream = [pscustomobject]@{
                Folder                = 'downstream'
                Source                = 'user'
                EffectiveChangeType   = 'patch'
                EffectiveTargetVersion = '0.1.1'
                CurrentVersion        = '0.1.0'
                AutoUpgraded          = $false
                CascadeReasons        = @()
            }
        }
        $snap = @{ upstream = 1; downstream = 2 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet $rs `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Folder | Should -Contain 'upstream'
        $findings.Folder | Should -Not -Contain 'downstream'
        $up = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $up.DependencyChains[0] | Should -Be @('downstream', 'upstream')
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
        # Linear2: only upstream changed; downstream is a user-source release-set
        # member (the user has already decided to release it). Both members carry
        # modifications past their baselines. Without the switch, only downstream
        # is a BFS root and surfaces upstream via 'downstream -> upstream'. With
        # the switch, upstream is also a BFS root (no deps, no extra chains) and
        # Phase B skips it (already a finding). downstream is excluded from the
        # Phase B sweep in both modes because it is user-source. Both calls
        # should produce the same single finding with the same chain.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'iamar-regression')
        $rs = @{
            downstream = [pscustomobject]@{
                Folder                = 'downstream'
                Source                = 'user'
                EffectiveChangeType   = 'patch'
                EffectiveTargetVersion = '0.1.1'
                CurrentVersion        = '0.1.0'
                AutoUpgraded          = $false
                CascadeReasons        = @()
            }
        }
        $snap = @{ upstream = 1; downstream = 1 }
        $without = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet $rs -ModifiedSnapshot $snap)
        $with = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet $rs -ModifiedSnapshot $snap `
            -IncludeAllModifiedAsRoots)

        $without.Count | Should -Be 1
        $with.Count    | Should -Be 1
        $without[0].Folder | Should -Be 'upstream'
        $with[0].Folder    | Should -Be 'upstream'
        $without[0].DependencyChains[0] | Should -Be @('downstream', 'upstream')
        $with[0].DependencyChains[0]    | Should -Be @('downstream', 'upstream')
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
        # Linear2: downstream → upstream. With -IncludeAllModifiedAsRoots the
        # changed downstream surfaces as a stub finding. Nothing else in the
        # workspace depends on downstream, so WorkspaceDependencyChains is @().
        # The menu will render "no in-workspace dependents" for it.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'wdc-leaf')
        $snap = @{ downstream = 1 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 1
        $findings[0].Folder | Should -Be 'downstream'
        @($findings[0].WorkspaceDependencyChains).Count | Should -Be 0
    }

    It 'is independent of release-set membership: same chain whether or not the dependent is in the release set' {
        Reset-ReleaseScriptCaches
        # Linear2: downstream → upstream. Modify upstream in earlier PR; do
        # NOT change downstream (i.e. downstream is NOT in the release set
        # via the BaseRef helper). With -IncludeAllModifiedAsRoots upstream
        # surfaces as a stub (DependencyChains is empty because no release
        # set member depends on it), but WorkspaceDependencyChains must still
        # list [downstream, upstream] — the big-picture view ignores
        # release-set membership.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'wdc-rs-independent')
        $ws.ModifySource('upstream')
        $ws.AddCommit('earlier PR: upstream edit')

        $snap = @{ upstream = 1 }
        $findings = @(Get-UnreleasedModifiedDependencies `
            -RepoRoot $ws.Path -ResolvedReleaseSet @{} `
            -ModifiedSnapshot $snap -IncludeAllModifiedAsRoots)

        $findings.Count | Should -Be 1
        $up = $findings[0]
        $up.Folder | Should -Be 'upstream'
        # Release-set-rooted DependencyChains is empty (no release-set member
        # depends on upstream — release set is empty here).
        @($up.DependencyChains).Count | Should -Be 0
        # Workspace-wide chains list the full graph path regardless.
        @($up.WorkspaceDependencyChains).Count | Should -Be 1
        $up.WorkspaceDependencyChains[0] | Should -Be @('downstream', 'upstream')
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
        $packageCargo = Join-Path $ws.Path 'crates\downstream\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        $new = Update-PackageVersion -packageName 'downstream' -version '0.1.1' -packageCargoToml $packageCargo -rootCargoToml $rootCargo
        $new | Should -Be '0.1.1'
        (Get-Content $packageCargo -Raw) | Should -Match 'version\s*=\s*"0\.1\.1"'
    }

    It 'updates the [workspace.dependencies] entry for the version-changed package' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-root')
        $packageCargo = Join-Path $ws.Path 'crates\upstream\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'upstream' -version '0.2.1' -packageCargoToml $packageCargo -rootCargoToml $rootCargo | Out-Null
        $rootContent = Get-Content $rootCargo -Raw
        $rootContent | Should -Match 'upstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.1"'
        # And downstream's version line in the same root table is unchanged.
        $rootContent | Should -Match 'downstream\s*=\s*\{[^}]*version\s*=\s*"0\.1\.0"'
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

        # Replace the downstream Cargo.toml with one that declares upstream inline
        # (instead of via .workspace = true).
        $downstreamCargo = Join-Path $ws.Path 'crates\downstream\Cargo.toml'
        Set-Content -Path $downstreamCargo -Value @"
[package]
name = "downstream"
version = "0.1.0"
edition = "2021"
publish = true

[lib]

[dependencies]
upstream = { path = "../upstream", version = "0.2.0" }
"@ -NoNewline

        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'downstream' -version '0.1.1' -packageCargoToml $downstreamCargo -rootCargoToml $rootCargo | Out-Null

        $content = Get-Content $downstreamCargo -Raw
        # [package] version was updated.
        $content | Should -Match 'name\s*=\s*"downstream"[^\[]*?version\s*=\s*"0\.1\.1"'
        # Inline upstream dep's declared version is preserved.
        if ($content -match 'upstream\s*=\s*\{[^}]*version\s*=\s*"([^"]+)"') {
            $Matches[1] | Should -Be '0.2.0' -Because 'Update-PackageVersion must not rewrite inline workspace-dep versions.'
        } else {
            throw "Could not extract upstream version from rewritten Cargo.toml: $content"
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

        $packageCargo = Join-Path $ws.Path 'crates\downstream\Cargo.toml'
        Set-Content -Path $packageCargo -NoNewline -Value @"
[package]
name = "downstream"
rust-version = "1.88"
version = "0.1.0"
edition = "2021"
publish = true

[lib]
"@

        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'downstream' -version '0.1.1' -packageCargoToml $packageCargo -rootCargoToml $rootCargo | Out-Null

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
        $upstreamChangelog   = Join-Path $ws.Path 'crates\upstream\CHANGELOG.md'
        $downstreamChangelog = Join-Path $ws.Path 'crates\downstream\CHANGELOG.md'
        $changelogBody = @(
            '# Changelog',
            '',
            '## [Unreleased]',
            '',
            '- manually curated note',
            ''
        ) -join "`n"
        Set-Content -LiteralPath $upstreamChangelog   -Value $changelogBody -NoNewline
        Set-Content -LiteralPath $downstreamChangelog -Value $changelogBody -NoNewline

        # Touch each package with a conventional-commit-formatted message so
        # Write-Changelog has something to fold into the new section — also
        # proves Write-Changelog ran (a no-modification call would early-
        # return with a warning and leave the CHANGELOG untouched).
        $ws.ModifySource('upstream',   '// upstream feature')
        $ws.AddCommit('feat(upstream): add upstream feature')
        $ws.ModifySource('downstream', '// downstream tweak')
        $ws.AddCommit('feat(downstream): use new upstream feature')

        $workspaceBaseline = @(Get-WorkspacePackages -repoRoot $ws.Path)
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'

        # Hand-build the ResolvedReleaseSet that Resolve-ReleaseSet would
        # produce for: -Packages upstream@non-breaking. On 0.x, non-breaking
        # collapses to a patch bump (0.2.0 -> 0.2.1). Cascade reaches
        # downstream as 'non-breaking' (no cargo_check_external_types declared
        # on either side, so the dep is treated as exposing; the upstream
        # non-breaking is not breaking, so the exposing change type carries
        # through to downstream non-breaking → 0.1.0 -> 0.1.1).
        $upstreamCascadeReasons   = New-Object 'System.Collections.Generic.List[object]'
        $downstreamCascadeReasons = New-Object 'System.Collections.Generic.List[object]'
        [void]$downstreamCascadeReasons.Add([pscustomobject]@{
            Target   = 'upstream'
            Version  = '0.2.1'
            Breaking = $false
        })

        $resolved = [ordered]@{
            upstream = [pscustomobject]@{
                Folder                   = 'upstream'
                Name                     = 'upstream'
                CurrentVersion           = '0.2.0'
                EffectiveTargetVersion   = '0.2.1'
                EffectiveChangeType      = 'non-breaking'
                Source                   = 'user'
                AutoUpgraded             = $false
                PinHonoredAgainstCascade = $false
                CascadeReasons           = $upstreamCascadeReasons
            }
            downstream = [pscustomobject]@{
                Folder                   = 'downstream'
                Name                     = 'downstream'
                CurrentVersion           = '0.1.0'
                EffectiveTargetVersion   = '0.1.1'
                EffectiveChangeType      = 'non-breaking'
                Source                   = 'cascade'
                AutoUpgraded             = $false
                PinHonoredAgainstCascade = $false
                CascadeReasons           = $downstreamCascadeReasons
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
        $releases[0].Package    | Should -Be 'upstream'
        $releases[0].OldVersion | Should -Be '0.2.0'
        $releases[0].NewVersion | Should -Be '0.2.1'
        $releases[1].Package    | Should -Be 'downstream'
        $releases[1].OldVersion | Should -Be '0.1.0'
        $releases[1].NewVersion | Should -Be '0.1.1'

        # --- Per-package Cargo.toml: the [package] version line is rewritten,
        # other [package] fields are preserved verbatim.
        $upstreamCargo   = Get-Content (Join-Path $ws.Path 'crates\upstream\Cargo.toml') -Raw
        $downstreamCargo = Get-Content (Join-Path $ws.Path 'crates\downstream\Cargo.toml') -Raw
        $upstreamCargo   | Should -Match '(?m)^version\s*=\s*"0\.2\.1"'
        $upstreamCargo   | Should -Not -Match '(?m)^version\s*=\s*"0\.2\.0"'
        $upstreamCargo   | Should -Match '(?m)^name\s*=\s*"upstream"'
        $downstreamCargo | Should -Match '(?m)^version\s*=\s*"0\.1\.1"'
        $downstreamCargo | Should -Not -Match '(?m)^version\s*=\s*"0\.1\.0"'
        $downstreamCargo | Should -Match '(?m)^name\s*=\s*"downstream"'
        # Dependency declaration in downstream Cargo.toml is preserved (workspace inheritance).
        $downstreamCargo | Should -Match '(?m)^upstream\.workspace\s*=\s*true'

        # --- Root Cargo.toml: both [workspace.dependencies] entries are updated.
        $rootContent = Get-Content $rootCargo -Raw
        $rootContent | Should -Match '(?m)^upstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.1"'
        $rootContent | Should -Match '(?m)^downstream\s*=\s*\{[^}]*version\s*=\s*"0\.1\.1"'
        $rootContent | Should -Not -Match '(?m)^upstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.0"'
        $rootContent | Should -Not -Match '(?m)^downstream\s*=\s*\{[^}]*version\s*=\s*"0\.1\.0"'

        # --- Per-package CHANGELOG: new version section was prepended.
        $today = (Get-Date).ToString('yyyy-MM-dd')
        $upstreamChangelogText   = Get-Content $upstreamChangelog   -Raw
        $downstreamChangelogText = Get-Content $downstreamChangelog -Raw

        # Top-level `# Changelog` header is preserved.
        $upstreamChangelogText   | Should -Match '(?m)^# Changelog'
        $downstreamChangelogText | Should -Match '(?m)^# Changelog'

        # New version section header (with today's date) appears in both.
        $upstreamChangelogText   | Should -Match ('(?m)^## \[0\.2\.1\] - ' + [regex]::Escape($today))
        $downstreamChangelogText | Should -Match ('(?m)^## \[0\.1\.1\] - ' + [regex]::Escape($today))

        # The manually-curated `## [Unreleased]` body line was folded into
        # the new version section (and the now-empty Unreleased heading was
        # stripped — Extract-UnreleasedSection consumed it).
        $upstreamChangelogText   | Should -Match 'manually curated note'
        $downstreamChangelogText | Should -Match 'manually curated note'
        $upstreamChangelogText   | Should -Not -Match '(?m)^## \[Unreleased\]'
        $downstreamChangelogText | Should -Not -Match '(?m)^## \[Unreleased\]'

        # Conventional-commit bullets from the feat(...) commits are grouped
        # under a `Features` section header.
        $upstreamChangelogText   | Should -Match 'Features'
        $upstreamChangelogText   | Should -Match 'add upstream feature'
        $downstreamChangelogText | Should -Match 'Features'
        $downstreamChangelogText | Should -Match 'use new upstream feature'

        # downstream is cascade-from-dependency: a Maintenance section with
        # a `Now requires <version> of <target>` bullet must be emitted even
        # though the package only had a feat commit (cascade bullets live in
        # their own section, separate from the conventional-commit ones).
        $downstreamChangelogText | Should -Match '🔧 Maintenance'
        $downstreamChangelogText | Should -Match 'Now requires `0\.2\.1` of `upstream`'

        # --- Update-Readme: invoked once per release-set member, with the
        # right per-package arguments. This is the README half of the
        # atomicity contract — Update-Readme is the only per-folder side
        # effect that doesn't produce an on-disk artefact in this fixture
        # (no README.j2 template, so the real implementation warns and
        # returns), and asserting the call count + arguments closes the
        # "wrote Cargo.toml but skipped README regen" regression mode.
        Should -Invoke -CommandName Update-Readme -Times 2 -Exactly
        Should -Invoke -CommandName Update-Readme -Times 1 -Exactly `
            -ParameterFilter { $packageName -eq 'upstream' }
        Should -Invoke -CommandName Update-Readme -Times 1 -Exactly `
            -ParameterFilter { $packageName -eq 'downstream' }

        # No README.md was written by the real path either (no template).
        (Test-Path (Join-Path $ws.Path 'crates\upstream\README.md'))   | Should -BeFalse
        (Test-Path (Join-Path $ws.Path 'crates\downstream\README.md')) | Should -BeFalse
    }
}

