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
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
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

    It 'N4 — bump-then-edit upstream is flagged via per-crate baseline' {
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
        # Build a workspace where 'upstream' starts as publish=false with pre-flip
        # edits, then is flipped to publish=true on a later commit. Current PR bumps
        # downstream only; pre-flip edits must not be reported.
        $spec = @{
            Crates = @(
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
        # No findings: per-crate baseline for upstream is the publish-flip commit,
        # newer than the pre-flip edit, so no unreleased changes.
        $findings.Count | Should -Be 0
    }

    It 'N8 — working-tree edits on upstream are flagged' {
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'n9')
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('bump downstream')
        Set-Content -Path (Join-Path $ws.Path 'crates\upstream\src\extra.rs') -Value '// new'

        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $ws.Path -BaseRef 'HEAD~1')
        $findings.Folder | Should -Contain 'upstream'
    }

    It 'T6b — dev-only dep on a modified crate is NOT flagged' {
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
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

    It 'Detached — modified crate in component B does not surface from a release in component A' {
        Invalidate-WorkspaceMetadataCache
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
        Invalidate-WorkspaceMetadataCache
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
}

# --------------------------------------------------------------------------
# Update-CrateVersion — exercise the [package]-scoped replacement.
# --------------------------------------------------------------------------

Describe 'Update-CrateVersion' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    It 'updates the crate version in its own Cargo.toml' {
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-basic')
        $crateCargo = Join-Path $ws.Path 'crates\downstream\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        $new = Update-CrateVersion -crateName 'downstream' -version '0.1.1' -bump '' -crateCargoToml $crateCargo -rootCargoToml $rootCargo
        $new | Should -Be '0.1.1'
        (Get-Content $crateCargo -Raw) | Should -Match 'version\s*=\s*"0\.1\.1"'
    }

    It 'updates the [workspace.dependencies] entry for the bumped crate' {
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-root')
        $crateCargo = Join-Path $ws.Path 'crates\upstream\Cargo.toml'
        $rootCargo  = Join-Path $ws.Path 'Cargo.toml'
        Update-CrateVersion -crateName 'upstream' -version '0.2.1' -bump '' -crateCargoToml $crateCargo -rootCargoToml $rootCargo | Out-Null
        $rootContent = Get-Content $rootCargo -Raw
        $rootContent | Should -Match 'upstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.1"'
        # And downstream's version line in the same root table is unchanged.
        $rootContent | Should -Match 'downstream\s*=\s*\{[^}]*version\s*=\s*"0\.1\.0"'
    }

    It 'preserves inline dependency version when the [package] version is bumped' {
        # Earlier, the crate-level regex was `(?<=version\s*=\s*")[^"]+` applied
        # via `-replace`, which clobbers every `version = "..."` in the file —
        # including any inline workspace-dep declarations like
        # `dep = { path = "...", version = "x.y.z" }`. Phase 8 fix scopes the
        # replacement to the [package] table only; this test pins the corrected
        # behavior.
        Invalidate-WorkspaceMetadataCache
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
        Update-CrateVersion -crateName 'downstream' -version '0.1.1' -bump '' -crateCargoToml $downstreamCargo -rootCargoToml $rootCargo | Out-Null

        $content = Get-Content $downstreamCargo -Raw
        # [package] version was bumped.
        $content | Should -Match 'name\s*=\s*"downstream"[^\[]*?version\s*=\s*"0\.1\.1"'
        # Inline upstream dep's declared version is preserved.
        if ($content -match 'upstream\s*=\s*\{[^}]*version\s*=\s*"([^"]+)"') {
            $Matches[1] | Should -Be '0.2.0' -Because 'Update-CrateVersion must not rewrite inline workspace-dep versions.'
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
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'uvc-rust-version')

        $crateCargo = Join-Path $ws.Path 'crates\downstream\Cargo.toml'
        Set-Content -Path $crateCargo -NoNewline -Value @"
[package]
name = "downstream"
rust-version = "1.88"
version = "0.1.0"
edition = "2021"
publish = true

[lib]
"@

        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Update-CrateVersion -crateName 'downstream' -version '0.1.1' -bump '' -crateCargoToml $crateCargo -rootCargoToml $rootCargo | Out-Null

        $content = Get-Content $crateCargo -Raw
        $content | Should -Match 'rust-version\s*=\s*"1\.88"' -Because 'rust-version must be left alone.'
        $content | Should -Match '(?m)^[ \t]*version\s*=\s*"0\.1\.1"'
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
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-fresh')
        # Initial commit is the base; downstream starts at 0.1.0.
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Push-Location $ws.Path
        try {
            $result = Invoke-CascadeStep -Dependent 'downstream' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetCrateName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'patch' -BaseRef 'HEAD'
        } finally {
            Pop-Location
        }

        $result | Should -Not -BeNullOrEmpty
        $result.Crate      | Should -Be 'downstream'
        $result.OldVersion | Should -Be '0.1.0'
        $result.NewVersion | Should -Be '0.1.1'
    }

    It 'skips re-bumping when the dependent was already pre-bumped to a sufficient version' {
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-skip')
        # Pre-bump downstream from 0.1.0 to 0.2.0 (larger than any patch cascade).
        $ws.BumpVersion('downstream', '0.2.0')
        $ws.AddCommit('pre-bump downstream')
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Push-Location $ws.Path
        try {
            # BaseRef = HEAD~1 → downstream's "base" version is 0.1.0; required = 0.1.1; current = 0.2.0.
            $result = Invoke-CascadeStep -Dependent 'downstream' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetCrateName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'patch' -BaseRef 'HEAD~1'
        } finally {
            Pop-Location
        }

        $result.OldVersion | Should -Be '0.2.0'
        $result.NewVersion | Should -Be '0.2.0'
    }

    It 'upgrades a pre-bumped dependent when the cascade requires a larger version' {
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-upgrade')
        # Pre-bump downstream to 0.1.1 (patch). Then cascade demands major (0.x → 0.2.0).
        $ws.BumpVersion('downstream', '0.1.1')
        $ws.AddCommit('pre-bump downstream patch')
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        Push-Location $ws.Path
        try {
            # Base ref = HEAD~1; base version = 0.1.0; required = Get-NextVersion(0.1.0, major) = 0.2.0.
            $result = Invoke-CascadeStep -Dependent 'downstream' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetCrateName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'major' -BaseRef 'HEAD~1'
        } finally {
            Pop-Location
        }

        $result.OldVersion | Should -Be '0.1.1'
        $result.NewVersion | Should -Be '0.2.0'
    }

    It 'returns null and warns when the dependent crate is missing' {
        Invalidate-WorkspaceMetadataCache
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'cs-missing')
        $rootCargo = Join-Path $ws.Path 'Cargo.toml'
        $warnings = @()
        Push-Location $ws.Path
        try {
            $result = Invoke-CascadeStep -Dependent 'nonexistent' -RepoRoot $ws.Path -RootCargoToml $rootCargo `
                -PrBaseUrl '' -TargetCrateName 'upstream' -TargetNewVersion '0.3.0' -DepBump 'patch' -BaseRef 'HEAD' `
                -WarningVariable warnings -WarningAction SilentlyContinue
        } finally {
            Pop-Location
        }
        $result | Should -BeNullOrEmpty
        $warnings.Count | Should -BeGreaterOrEqual 1
    }
}
