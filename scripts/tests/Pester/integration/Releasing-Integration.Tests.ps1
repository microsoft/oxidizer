# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Phase 5 — integration tests for the analyses that orchestrate multiple
# helpers. Each test uses a tiny synthetic Cargo workspace and exercises a
# realistic interplay between version bumps, source edits, and the
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

    It 'N1 — modified upstream + bumped downstream in same PR is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n1')
        # Earlier baseline = initial commit. In this PR: modify upstream + bump downstream.
        $ws.ModifySource('upstream')
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('PR commit')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Count | Should -Be 1
        $findings[0].Folder | Should -Be 'upstream'
        $findings[0].DependencyChains[0] | Should -Be @('downstream', 'upstream')
        # CurrentVersion threads through from cargo metadata so the menu can
        # render concrete version transitions (e.g. "0.2.0 -> 0.3.0").
        $findings[0].CurrentVersion | Should -Be '0.2.0'
    }

    It 'N2 — earlier-PR upstream edit + current-PR downstream bump is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n2')
        # Simulate previous PR landing an upstream edit without a bump:
        $ws.ModifySource('upstream')
        $ws.AddCommit('previous PR: upstream edit')
        # Current PR bumps downstream only:
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('current PR: downstream bump')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'N3 — upstream already bumped cleanly; no further edits → no finding' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n3')
        # Previous PR: bump upstream and release.
        $ws.BumpVersion('upstream', '0.2.1')
        $ws.AddCommit('release upstream 0.2.1')
        # Current PR: bump downstream only.
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('release downstream 0.1.1')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Count | Should -Be 0
    }

    It 'N4 — bump-then-edit upstream is flagged via per-package baseline' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n4')
        # Earlier: bump upstream + release.
        $ws.BumpVersion('upstream', '0.2.1')
        $ws.AddCommit('release upstream 0.2.1')
        # Later: edit upstream source (no bump).
        $ws.ModifySource('upstream')
        $ws.AddCommit('post-release upstream edit')
        # Current PR: bump downstream only.
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('release downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'N5 — BFS reaches a modified leaf through an unchanged middle' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'n5')
        # Modify the deepest leaf 'c' in an earlier PR.
        $ws.ModifySource('c')
        $ws.AddCommit('previous PR: c edit')
        # Current PR: bump 'a' only. Middle 'b' is unchanged.
        $ws.BumpVersion('a', '0.1.1')
        $ws.AddCommit('current PR: bump a')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
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
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('current PR: bump downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'N7 — publish=false → true flip resets the baseline (pre-flip edits ignored)' {
        Reset-ReleaseScriptCaches
        # Build a workspace where 'upstream' starts as publish=false with pre-flip
        # edits, then is flipped to publish=true on a later commit. Current PR bumps
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
        # Current PR: bump downstream only.
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('release downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        # No findings: per-package baseline for upstream is the publish-flip commit,
        # newer than the pre-flip edit, so no unreleased changes.
        $findings.Count | Should -Be 0
    }

    It 'N8 — working-tree edits on upstream are flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n8')
        # Current PR: bump downstream (committed).
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('bump downstream')
        # Uncommitted: tweak upstream source.
        $ws.ModifySource('upstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'N9 — untracked new file in upstream is flagged' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n9')
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('bump downstream')
        Set-Content -Path (Join-Path $ws.Path 'crates\upstream\src\extra.rs') -Value '// new'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'T6b — dev-only dep on a modified package is NOT flagged' {
        Reset-ReleaseScriptCaches
        # Mixed6's 'target' has a dev-dep on upstream_a (normal dep on upstream_b).
        $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 't6b')
        $ws.ModifySource('upstream_a')
        $ws.AddCommit('upstream_a edit')
        $ws.BumpVersion('target', '0.1.1')
        $ws.AddCommit('bump target')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Not -Contain 'upstream_a'
    }

    It 'T15 — publish=false dep is NOT flagged even when modified' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 't15')
        # 'utility' is publish=false. Modify it and bump downstream_y which depends on it.
        $ws.ModifySource('utility')
        $ws.AddCommit('utility edit')
        $ws.BumpVersion('downstream_y', '0.5.1')
        $ws.AddCommit('bump downstream_y')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Not -Contain 'utility'
    }

    It 'T16-style aggregation — one shared upstream across multiple bumped downstreams gets multiple chains' {
        Reset-ReleaseScriptCaches
        # Diamond4: top -> {left, right}; left -> bottom; right -> bottom.
        # Modify bottom in an earlier PR; bump both left and right.
        $ws = New-SyntheticWorkspace -Preset Diamond4 -Path (Join-Path $TestDrive 't16-style')
        $ws.ModifySource('bottom')
        $ws.AddCommit('previous PR: bottom edit')
        $ws.BumpVersion('left',  '0.2.1')
        $ws.BumpVersion('right', '0.3.1')
        $ws.AddCommit('current PR: bump left + right')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $bottom = $findings | Where-Object { $_.Folder -eq 'bottom' }
        $bottom | Should -Not -BeNullOrEmpty
        @($bottom.DependencyChains).Count | Should -Be 2
    }

    It 'Detached — modified package in component B does not surface from a release in component A' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Detached -Path (Join-Path $TestDrive 'detached')
        # Two disconnected components: alpha→beta and gamma→delta.
        # Modify 'gamma' (component B) and bump 'alpha' (component A).
        $ws.ModifySource('gamma')
        $ws.AddCommit('mod gamma')
        $ws.BumpVersion('alpha', '0.1.1')
        $ws.AddCommit('bump alpha')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
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
        $ws.BumpVersion('a', '0.1.1')
        $ws.BumpVersion('b', '0.2.1')
        $ws.AddCommit('current PR: bump a + b')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Be 'c'
        $cFinding = $findings | Where-Object { $_.Folder -eq 'c' }
        @($cFinding.DependencyChains).Count | Should -Be 1
        @($cFinding.DependencyChains)[0] -join ',' | Should -Be 'a,b,c'
    }

    It 'tags non-release-set findings with InReleaseSet = $false' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'irs-classic')
        $ws.ModifySource('upstream')
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('mod upstream + bump downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeFalse
    }
}

# --------------------------------------------------------------------------
# Get-UnreleasedModifiedDependencies — Invariant B (release-set members
# whose cascade-applied bump is below "breaking" must surface as elevation
# candidates) and the -ModifiedSnapshot mechanism (Invariant A: cascade
# writes must not pollute the working-tree query).
# --------------------------------------------------------------------------

Describe 'Get-UnreleasedModifiedDependencies: release-set elevation (Invariant B)' {

    # Helper: build a Linear2 workspace where 'upstream' is BOTH a release-set
    # member (its version differs from BaseRef) AND has unreleased
    # modifications past its per-package baseline. We arrange this by:
    #   HEAD~2 → initial (upstream at 0.2.0)
    #   HEAD~1 → upstream bumped to $upstreamPending (this becomes upstream's
    #            per-package baseline; release-set membership against
    #            BaseRef=HEAD~2 depends on the version differing)
    #   HEAD   → source edit on upstream + bump downstream so the loop has
    #            something to traverse from. Now upstream is in the release
    #            set AND has modifications post-baseline.
    function script:NewElevationWorkspace {
        param(
            [string]$Path,
            [string]$UpstreamPending  # the in-PR pending version for upstream
        )
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path $Path
        $ws.BumpVersion('upstream', $UpstreamPending)
        $ws.AddCommit('bump upstream (pending release)')
        $ws.ModifySource('upstream', '// post-bump edit, may warrant elevation')
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('mod upstream + bump downstream')
        return $ws
    }

    It 'surfaces a release-set member whose cascade-applied bump is patch (0.x — Invariant B)' {
        Reset-ReleaseScriptCaches
        # upstream goes 0.2.0 → 0.2.1 (patch); per Test-IsBreakingChange this
        # is non-breaking, so the user should be prompted to elevate.
        $ws = NewElevationWorkspace -Path (Join-Path $TestDrive 'irs-patch') -UpstreamPending '0.2.1'
        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2')
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeTrue
        $u.CurrentVersion | Should -Be '0.2.1'
    }

    It 'does NOT surface a release-set member whose cascade-applied bump is breaking (0.x major)' {
        Reset-ReleaseScriptCaches
        # upstream goes 0.2.0 → 0.3.0 (major-on-0.x, i.e. breaking per
        # Test-IsBreakingChange) — no further elevation is possible, so the
        # user should not be prompted.
        $ws = NewElevationWorkspace -Path (Join-Path $TestDrive 'irs-major0x') -UpstreamPending '0.3.0'
        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2')
        $findings | Where-Object { $_.Folder -eq 'upstream' } | Should -BeNullOrEmpty
    }

    It 'surfaces a release-set member whose cascade-applied bump is non-breaking on 1.x' {
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
        $ws.BumpVersion('upstream', '1.3.0')
        $ws.AddCommit('pending minor release of upstream')
        $ws.ModifySource('upstream', '// post-bump edit')
        $ws.BumpVersion('downstream', '1.0.1')
        $ws.AddCommit('mod upstream + bump downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2')
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet   | Should -BeTrue
        $u.CurrentVersion | Should -Be '1.3.0'
    }

    It 'does NOT surface a release-set member whose cascade-applied bump is breaking on 1.x' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'irs-1x-major')
        $ws.BumpVersion('upstream', '2.0.0')
        $ws.AddCommit('pending major release of upstream')
        $ws.ModifySource('upstream', '// post-bump edit')
        $ws.BumpVersion('downstream', '1.0.1')
        $ws.AddCommit('mod upstream + bump downstream')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2')
        $findings | Where-Object { $_.Folder -eq 'upstream' } | Should -BeNullOrEmpty
    }

    It 'still surfaces a release-set member whose pending bump is patch, even when only the working tree carries the modifications' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'irs-worktree')
        $ws.BumpVersion('upstream', '0.2.1')
        $ws.AddCommit('pending patch release of upstream')
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('bump downstream')
        # Uncommitted source edit on upstream — past its per-package baseline.
        $ws.ModifySource('upstream', '// uncommitted further edit')

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~2')
        $u = $findings | Where-Object { $_.Folder -eq 'upstream' }
        $u | Should -Not -BeNullOrEmpty
        $u.InReleaseSet | Should -BeTrue
    }
}

Describe 'Get-UnreleasedModifiedDependencies: -ModifiedSnapshot honored (Invariant A)' {

    It 'uses the caller-provided snapshot instead of querying the working tree' {
        Reset-ReleaseScriptCaches
        # Build a workspace where the working tree has NO unreleased
        # modifications on upstream — only a pending downstream bump.
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'ms-fake-snap')
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('pending downstream bump')

        # Without a snapshot: the live query finds nothing on upstream.
        $live = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $live.Folder | Should -Not -Contain 'upstream'

        # With a synthetic snapshot claiming upstream IS modified, the BFS
        # surfaces it as a classic (non-release-set) finding.
        $snap = @{ 'upstream' = 3 }
        $with = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1' -ModifiedSnapshot $snap)
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
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('mod upstream + bump downstream')

        # Sanity check that the live query DOES find it.
        $live = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $live.Folder | Should -Contain 'upstream'

        # With an empty snapshot, the BFS surfaces nothing.
        $with = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1' -ModifiedSnapshot @{})
        $with.Count | Should -Be 0
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
        $new = Update-PackageVersion -packageName 'downstream' -version '0.1.1' -bump '' -packageCargoToml $packageCargo -rootCargoToml $rootCargo
        $new | Should -Be '0.1.1'
        (Get-Content $packageCargo -Raw) | Should -Match 'version\s*=\s*"0\.1\.1"'
    }

    It 'updates the [workspace.dependencies] entry for the bumped package' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-root')
        $packageCargo = Join-Path $ws.Path 'crates\upstream\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        Update-PackageVersion -packageName 'upstream' -version '0.2.1' -bump '' -packageCargoToml $packageCargo -rootCargoToml $rootCargo | Out-Null
        $rootContent = Get-Content $rootCargo -Raw
        $rootContent | Should -Match 'upstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.1"'
        # And downstream's version line in the same root table is unchanged.
        $rootContent | Should -Match 'downstream\s*=\s*\{[^}]*version\s*=\s*"0\.1\.0"'
    }

    It 'preserves inline dependency version when the [package] version is bumped' {
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
        Update-PackageVersion -packageName 'downstream' -version '0.1.1' -bump '' -packageCargoToml $downstreamCargo -rootCargoToml $rootCargo | Out-Null

        $content = Get-Content $downstreamCargo -Raw
        # [package] version was bumped.
        $content | Should -Match 'name\s*=\s*"downstream"[^\[]*?version\s*=\s*"0\.1\.1"'
        # Inline upstream dep's declared version is preserved.
        if ($content -match 'upstream\s*=\s*\{[^}]*version\s*=\s*"([^"]+)"') {
            $Matches[1] | Should -Be '0.2.0' -Because 'Update-PackageVersion must not rewrite inline workspace-dep versions.'
        } else {
            throw "Could not extract upstream version from rewritten Cargo.toml: $content"
        }
    }

    It 'preserves rust-version when the [package] version is bumped' {
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
        Update-PackageVersion -packageName 'downstream' -version '0.1.1' -bump '' -packageCargoToml $packageCargo -rootCargoToml $rootCargo | Out-Null

        $content = Get-Content $packageCargo -Raw
        $content | Should -Match 'rust-version\s*=\s*"1\.88"' -Because 'rust-version must be left alone.'
        $content | Should -Match '(?m)^[ \t]*version\s*=\s*"0\.1\.1"'
    }
}

# --------------------------------------------------------------------------
# Update-PackageVersion — user-visible "Releasing <change-type>:" announcement.
# --------------------------------------------------------------------------
#
# Regression guard for a UX bug where the function emitted
# "✅ Incrementing $bumpType version from <old> to <new>." with $bumpType set
# to the internal Cargo bump-kind enum ('major'/'minor'/'patch'). On 0.x.y
# packages a 'major' bump moves the MINOR version component (e.g.
# 0.4.1 -> 0.5.0), so labeling it a "major version" increment was wrong on
# two counts: (1) the *major component* (the leading 0) did not change, and
# (2) the internal enum is a change-TYPE label, not a version-component name.
#
# Per AGENTS.md "Release Versioning Vocabulary", user-visible output must use
# change-type vocabulary (breaking change / non-breaking change / patch) and
# never leak the internal bump-kind enum.

Describe 'Update-PackageVersion: user-visible change-type vocabulary' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    # Helper: seed a synthetic Linear2 workspace and force `downstream` to a
    # specific starting version so we can exercise both 0.x and 1.x rules.
    function script:NewBumpFixture {
        param([string]$Path, [string]$StartVersion)
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path $Path
        $packageCargo = Join-Path $ws.Path 'crates\downstream\Cargo.toml'
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        # Seed `downstream` to the requested starting version. Force-write
        # via Update-PackageVersion so root Cargo.toml stays in sync.
        Update-PackageVersion -packageName 'downstream' -version $StartVersion -bump '' `
            -packageCargoToml $packageCargo -rootCargoToml $rootCargo 6>$null | Out-Null
        return [pscustomobject]@{
            PackageCargo = $packageCargo
            RootCargo    = $rootCargo
        }
    }

    Context "internal bump-kind 'major'" {
        It 'on a 0.x.y package: announces a breaking change with the MINOR component bumped (regression guard)' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-major-0x') -StartVersion '0.4.1'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '' -bump 'major' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            # Numeric transition: 0.4.1 -> 0.5.0 (minor component moves on 0.x major bump).
            $script:new | Should -Be '0.5.0'
            $text | Should -Match 'Releasing breaking change: 0\.4\.1 -> 0\.5\.0'
            # Regression: the internal enum must not leak as "major version".
            $text | Should -Not -Match 'major version'
            $text | Should -Not -Match 'Incrementing'
        }

        It 'on a 1.x.y package: announces a breaking change with the MAJOR component bumped' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-major-1x') -StartVersion '1.2.3'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '' -bump 'major' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            $script:new | Should -Be '2.0.0'
            $text | Should -Match 'Releasing breaking change: 1\.2\.3 -> 2\.0\.0'
            $text | Should -Not -Match 'major version'
        }
    }

    Context "internal bump-kind 'minor'" {
        It 'on a 0.x.y package: announces a non-breaking change with the PATCH component bumped' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-minor-0x') -StartVersion '0.4.1'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '' -bump 'minor' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            $script:new | Should -Be '0.4.2'
            $text | Should -Match 'Releasing non-breaking change: 0\.4\.1 -> 0\.4\.2'
            $text | Should -Not -Match 'minor version'
        }

        It 'on a 1.x.y package: announces a non-breaking change with the MINOR component bumped' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-minor-1x') -StartVersion '1.2.3'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '' -bump 'minor' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            $script:new | Should -Be '1.3.0'
            $text | Should -Match 'Releasing non-breaking change: 1\.2\.3 -> 1\.3\.0'
            $text | Should -Not -Match 'minor version'
        }
    }

    Context "internal bump-kind 'patch'" {
        It 'on a 0.x.y package: announces a patch with the PATCH component bumped' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-patch-0x') -StartVersion '0.4.1'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '' -bump 'patch' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            $script:new | Should -Be '0.4.2'
            $text | Should -Match 'Releasing patch: 0\.4\.1 -> 0\.4\.2'
            $text | Should -Not -Match 'patch version'
        }

        It 'on a 1.x.y package: announces a patch with the PATCH component bumped' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-patch-1x') -StartVersion '1.2.3'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '' -bump 'patch' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            $script:new | Should -Be '1.2.4'
            $text | Should -Match 'Releasing patch: 1\.2\.3 -> 1\.2\.4'
            $text | Should -Not -Match 'patch version'
        }
    }

    Context 'implicit default bump (neither -version nor -bump supplied)' {
        It 'defaults to non-breaking change vocabulary (internal default is bump=minor)' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-default') -StartVersion '1.2.3'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '' -bump '' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            $script:new | Should -Be '1.3.0'
            $text | Should -Match 'Releasing non-breaking change: 1\.2\.3 -> 1\.3\.0'
        }
    }

    Context 'explicit -version (caller supplies the new version verbatim)' {
        It 'uses the "Using specified version:" wording and skips the change-type label' {
            $fx = NewBumpFixture -Path (Join-Path $TestDrive 'uvc-cv-explicit') -StartVersion '0.4.1'
            $new = $null
            $out = & {
                $script:new = Update-PackageVersion -packageName 'downstream' -version '9.9.9' -bump '' `
                    -packageCargoToml $fx.PackageCargo -rootCargoToml $fx.RootCargo
            } 6>&1
            $text = ($out | Out-String)

            $script:new | Should -Be '9.9.9'
            $text | Should -Match 'Using specified version: 9\.9\.9'
            # When the version is explicit there is no change-type to announce.
            $text | Should -Not -Match 'Releasing (breaking|non-breaking|patch)'
        }
    }
}

# --------------------------------------------------------------------------
# Invoke-CascadeStep — re-bump-safe behavior in isolation.
# --------------------------------------------------------------------------

Describe 'Invoke-CascadeStep' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    It 'bumps a dependent from the base version when not yet pre-bumped' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-fresh')
        # Initial commit is the base; downstream starts at 0.1.0.
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Push-Location $ws.Path
        try {
            $result = Invoke-CascadeStep -Dependent 'downstream' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetPackageName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'patch' -BaseRef 'HEAD'
        } finally {
            Pop-Location
        }

        $result | Should -Not -BeNullOrEmpty
        $result.Package      | Should -Be 'downstream'
        $result.OldVersion | Should -Be '0.1.0'
        $result.NewVersion | Should -Be '0.1.1'
    }

    It 'skips re-bumping when the dependent was already pre-bumped to a sufficient version' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-skip')
        # Pre-bump downstream from 0.1.0 to 0.2.0 (larger than any patch cascade).
        $ws.BumpVersion('downstream', '0.2.0')
        $ws.AddCommit('pre-bump downstream')
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Push-Location $ws.Path
        try {
            # BaseRef = HEAD~1 → downstream's "base" version is 0.1.0; required = 0.1.1; current = 0.2.0.
            $result = Invoke-CascadeStep -Dependent 'downstream' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetPackageName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'patch' -BaseRef 'HEAD~1'
        } finally {
            Pop-Location
        }

        $result.OldVersion | Should -Be '0.2.0'
        $result.NewVersion | Should -Be '0.2.0'
    }

    It 'upgrades a pre-bumped dependent when the cascade requires a larger version' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-upgrade')
        # Pre-bump downstream to 0.1.1 (patch). Then cascade demands major (0.x → 0.2.0).
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('pre-bump downstream patch')
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Push-Location $ws.Path
        try {
            # Base ref = HEAD~1; base version = 0.1.0; required = Get-NextVersion(0.1.0, major) = 0.2.0.
            $result = Invoke-CascadeStep -Dependent 'downstream' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetPackageName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'major' -BaseRef 'HEAD~1'
        } finally {
            Pop-Location
        }

        $result.OldVersion | Should -Be '0.1.1'
        $result.NewVersion | Should -Be '0.2.0'
    }

    It 'returns null and warns when the dependent package is missing' {
        Reset-ReleaseScriptCaches
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-missing')
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        $warnings = @()
        Push-Location $ws.Path
        try {
            $result = Invoke-CascadeStep -Dependent 'nonexistent' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetPackageName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'patch' -BaseRef 'HEAD' `
                -WarningVariable warnings -WarningAction SilentlyContinue
        } finally {
            Pop-Location
        }
        $result | Should -BeNullOrEmpty
        $warnings.Count | Should -BeGreaterOrEqual 1
    }
}
