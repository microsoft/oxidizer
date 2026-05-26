# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Phase 4 — git / filesystem unit tests. Each test builds a tiny synthetic
# workspace via New-SyntheticWorkspace and exercises one helper from
# scripts/lib/releasing.ps1 in isolation. The workspace metadata cache is
# invalidated between tests so a previous fixture cannot pollute the next.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
    . (Join-Path $env:OXI_TEST_COMMON 'New-SyntheticWorkspace.ps1')
}

Describe 'Test-GitRef' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'gitref')
    }

    It 'returns true for an existing branch' {
        Test-GitRef -Ref 'main' -RepoRoot $script:Ws.Path | Should -BeTrue
    }

    It 'returns true for HEAD' {
        Test-GitRef -Ref 'HEAD' -RepoRoot $script:Ws.Path | Should -BeTrue
    }

    It 'returns false for a non-existent ref' {
        Test-GitRef -Ref 'origin/main' -RepoRoot $script:Ws.Path | Should -BeFalse
        Test-GitRef -Ref 'no-such-branch' -RepoRoot $script:Ws.Path | Should -BeFalse
    }

    It 'returns true for a SHA' {
        $sha = $script:Ws.GitSha('HEAD')
        Test-GitRef -Ref $sha -RepoRoot $script:Ws.Path | Should -BeTrue
    }
}

Describe 'Get-CurrentVersion' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'currentversion')
    }

    It 'reads the version from a crate Cargo.toml' {
        $cargo = Join-Path $script:Ws.Path 'crates\upstream\Cargo.toml'
        Get-CurrentVersion -cargoTomlPath $cargo | Should -Be '0.2.0'
    }

    It 'throws on a missing file' {
        { Get-CurrentVersion -cargoTomlPath (Join-Path $script:Ws.Path 'no.toml') } | Should -Throw
    }

    It 'ignores rust-version when it appears in the [package] table' {
        # Substring keys like `rust-version = "1.88"` must not be mistaken for the
        # package's `version = "..."` literal. Writes a custom Cargo.toml with
        # rust-version placed BEFORE version to stress the regex.
        $tomlPath = Join-Path $TestDrive 'rust-version-before.toml'
        Set-Content -Path $tomlPath -NoNewline -Value @'
[package]
name = "demo"
rust-version = "1.88"
version = "0.7.3"
edition = "2021"
'@
        Get-CurrentVersion -cargoTomlPath $tomlPath | Should -Be '0.7.3'
    }

    It 'ignores rust-version when it appears AFTER the version line' {
        $tomlPath = Join-Path $TestDrive 'rust-version-after.toml'
        Set-Content -Path $tomlPath -NoNewline -Value @'
[package]
name = "demo"
version = "0.7.3"
rust-version = "1.88"
edition = "2021"
'@
        Get-CurrentVersion -cargoTomlPath $tomlPath | Should -Be '0.7.3'
    }

    It 'tolerates bracket characters inside non-version fields above the version' {
        # description = "Helper for [foo]" must not interrupt the [package]-scoped
        # match; only an actual subtable header (^[...) should.
        $tomlPath = Join-Path $TestDrive 'bracketed-description.toml'
        Set-Content -Path $tomlPath -NoNewline -Value @'
[package]
name = "demo"
description = "Helper for [foo] bar"
version = "0.4.0"
'@
        Get-CurrentVersion -cargoTomlPath $tomlPath | Should -Be '0.4.0'
    }
}

Describe 'Get-CrateVersionFromRef' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'versionfromref')
        # Bump 'b' on a follow-up commit so we have two distinct versions in history.
        $script:Ws.BumpVersion('b', '0.2.1')
        $script:Ws.AddCommit('bump b')
    }

    It 'reads the version at HEAD' {
        Get-CrateVersionFromRef -RepoRoot $script:Ws.Path -BaseRef 'HEAD' -CrateFolder 'b' | Should -Be '0.2.1'
    }

    It 'reads the older version from a prior commit' {
        Get-CrateVersionFromRef -RepoRoot $script:Ws.Path -BaseRef 'HEAD~1' -CrateFolder 'b' | Should -Be '0.2.0'
    }

    It 'returns null when the crate does not exist at the ref' {
        Get-CrateVersionFromRef -RepoRoot $script:Ws.Path -BaseRef 'HEAD' -CrateFolder 'nonexistent' | Should -BeNullOrEmpty
    }
}

Describe 'Get-CrateLastReleaseBaseline' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'baseline')
        # Initial commit (call it C0). Now make a source-only edit (C1), then a
        # version-bumping commit (C2), then another source edit (C3). Baseline
        # for 'b' must be C2.
        $script:Ws.ModifySource('b')
        $script:Ws.AddCommit('source edit to b')
        $script:Sha_C1 = $script:Ws.GitSha('HEAD')

        $script:Ws.BumpVersion('b', '0.2.1')
        $script:Ws.AddCommit('bump b to 0.2.1')
        $script:Sha_C2 = $script:Ws.GitSha('HEAD')

        $script:Ws.ModifySource('b')
        $script:Ws.AddCommit('further source edit to b')
        $script:Sha_C3 = $script:Ws.GitSha('HEAD')
    }

    It 'returns the SHA of the most recent version-changing commit' {
        $sha = Get-CrateLastReleaseBaseline -RepoRoot $script:Ws.Path -CrateFolder 'b'
        $sha | Should -Be $script:Sha_C2
    }

    It 'returns null for a crate folder that has never existed' {
        Get-CrateLastReleaseBaseline -RepoRoot $script:Ws.Path -CrateFolder 'nonexistent' | Should -BeNullOrEmpty
    }
}

Describe 'Get-WorkspaceCrates' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'wscrates')
    }

    It 'returns one entry per workspace crate' {
        $crates = Get-WorkspaceCrates -repoRoot $script:Ws.Path
        $crates.Count | Should -Be 6
    }

    It 'reports Published=$false for publish=false crates' {
        $crates = Get-WorkspaceCrates -repoRoot $script:Ws.Path
        $util = $crates | Where-Object { $_.Name -eq 'utility' }
        $util.Published | Should -BeFalse
        ($crates | Where-Object { $_.Name -ne 'utility' } | ForEach-Object Published) | ForEach-Object { $_ | Should -BeTrue }
    }

    It 'excludes dev-deps from Deps' {
        $crates = Get-WorkspaceCrates -repoRoot $script:Ws.Path
        $target = $crates | Where-Object { $_.Name -eq 'target' }
        # Mixed6 wires a normal dep on upstream_b and a dev dep on upstream_a;
        # Get-WorkspaceCrates flattens to normal/build only.
        $target.Deps | Should -Contain 'upstream_b'
        $target.Deps | Should -Not -Contain 'upstream_a'
    }
}

Describe 'Get-AllTransitiveDependents' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Diamond4 -Path (Join-Path $TestDrive 'transitive-diamond')
        # Diamond4: top -> {left, right}; left -> bottom; right -> bottom. Dependents of bottom are {left, right, top}.
    }

    It 'finds dependents through both diamond legs (deduped)' {
        $deps = Get-AllTransitiveDependents -crateName 'bottom' -repoRoot $script:Ws.Path
        ($deps | Sort-Object) | Should -Be @('left', 'right', 'top')
    }

    It 'returns no dependents for a leaf (top) crate' {
        # 'top' is the top of the diamond; nothing depends on it.
        $deps = @(Get-AllTransitiveDependents -crateName 'top' -repoRoot $script:Ws.Path)
        $deps.Count | Should -Be 0
    }

    It 'excludes publish=false crates from the result' {
        # Use Mixed6 — utility is publish=false and depends on downstream_y.
        Invalidate-WorkspaceMetadataCache
        $mixed = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'transitive-mixed')
        $deps = Get-AllTransitiveDependents -crateName 'downstream_y' -repoRoot $mixed.Path
        # utility depends on downstream_y but is publish=false; should not appear.
        $deps | Should -Not -Contain 'utility'
    }
}

Describe 'Get-CratesWithUnreleasedChanges' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleasedchanges')
        # Edit crate b after initial commit and commit it.
        $script:Ws.ModifySource('b')
        $script:Ws.AddCommit('b edit')
    }

    It 'reports committed source edits as unreleased' {
        $changes = Get-CratesWithUnreleasedChanges -RepoRoot $script:Ws.Path
        $changes.ContainsKey('b') | Should -BeTrue
        $changes['b'] | Should -BeGreaterOrEqual 1
    }

    It 'reports working-tree edits as unreleased' {
        Invalidate-WorkspaceMetadataCache
        $w2 = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleasedworking')
        # Uncommitted source edit on c.
        $w2.ModifySource('c')
        $changes = Get-CratesWithUnreleasedChanges -RepoRoot $w2.Path
        $changes.ContainsKey('c') | Should -BeTrue
    }

    It 'reports untracked files as unreleased' {
        Invalidate-WorkspaceMetadataCache
        $w3 = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleaseduntracked')
        $newFile = Join-Path $w3.Path 'crates\a\src\new_file.rs'
        Set-Content -Path $newFile -Value '// new'
        $changes = Get-CratesWithUnreleasedChanges -RepoRoot $w3.Path
        $changes.ContainsKey('a') | Should -BeTrue
    }

    It 'skips publish=false crates' {
        Invalidate-WorkspaceMetadataCache
        $w4 = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'unreleasedmixed')
        $w4.ModifySource('utility')
        $w4.AddCommit('utility edit')
        $changes = Get-CratesWithUnreleasedChanges -RepoRoot $w4.Path
        $changes.ContainsKey('utility') | Should -BeFalse
    }
}

Describe 'Get-CratesWithVersionBumps' {
    BeforeAll {
        Invalidate-WorkspaceMetadataCache
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'versionbumps')
        # Bump 'a' relative to HEAD~ baseline.
        $script:Ws.BumpVersion('a', '0.1.1')
        $script:Ws.AddCommit('bump a')
    }

    It 'reports a crate whose version differs vs the base ref' {
        $bumped = Get-CratesWithVersionBumps -RepoRoot $script:Ws.Path -BaseRef 'HEAD~1'
        $bumped.Contains('a') | Should -BeTrue
        $bumped.Contains('b') | Should -BeFalse
        $bumped.Contains('c') | Should -BeFalse
    }

    It 'returns an empty set when the working tree matches the base ref' {
        Invalidate-WorkspaceMetadataCache
        $w2 = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'versionbumps-empty')
        $bumped = Get-CratesWithVersionBumps -RepoRoot $w2.Path -BaseRef 'HEAD'
        $bumped.Count | Should -Be 0
    }
}

Describe 'Add-CascadeBulletToVersionSection' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    BeforeEach {
        $script:Changelog = Join-Path $TestDrive ('cascade-' + [guid]::NewGuid().Guid.Substring(0,8) + '.md')
        Set-Content -Path $script:Changelog -Value @"
# Changelog

## [0.2.0] - 2025-01-01

- ✨ Features

  - already noted

"@
    }

    It 'inserts a Maintenance bullet for a non-breaking cascade' {
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.3.0'; Breaking = $false
        }
        $content = Get-Content $script:Changelog -Raw
        $content | Should -Match '- 🔧 Maintenance'
        $content | Should -Match 'Now requires `0.3.0` of `depcrate`'
        $content | Should -Not -Match '- ⚠️ Breaking'
    }

    It 'inserts a Breaking section bullet when the cascade is breaking' {
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'bigdep'; Version = '1.0.0'; Breaking = $true
        }
        $content = Get-Content $script:Changelog -Raw
        $content | Should -Match '- ⚠️ Breaking'
        $content | Should -Match 'Now requires `1.0.0` of `bigdep`'
    }

    It 'warns and returns when the target version section is missing' {
        $warnings = @()
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '9.9.9' -CascadeReason @{
            Target = 'x'; Version = '0.1.0'; Breaking = $false
        } -WarningVariable warnings -WarningAction SilentlyContinue
        $warnings.Count | Should -BeGreaterOrEqual 1
        ($warnings -join ' ') | Should -Match '9.9.9'
    }

    It 'warns and returns when the changelog file does not exist' {
        $warnings = @()
        Add-CascadeBulletToVersionSection -ChangelogFile (Join-Path $TestDrive 'missing.md') -Version '0.1.0' -CascadeReason @{
            Target = 'x'; Version = '0.1.0'; Breaking = $false
        } -WarningVariable warnings -WarningAction SilentlyContinue
        $warnings.Count | Should -BeGreaterOrEqual 1
    }
}
