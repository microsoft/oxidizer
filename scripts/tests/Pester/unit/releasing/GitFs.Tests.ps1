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
        Reset-ReleaseScriptCaches
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
        Reset-ReleaseScriptCaches
    $script:Ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'currentversion')
    }

    It 'reads the version from a package Cargo.toml' {
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

Describe 'Get-PackageVersionFromRef' {
    BeforeAll {
        Reset-ReleaseScriptCaches
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'versionfromref')
        # Change version of 'b' on a follow-up commit so we have two distinct versions in history.
        $script:Ws.SetVersion('b', '0.2.1')
        $script:Ws.AddCommit('change b')
    }

    It 'reads the version at HEAD' {
        Get-PackageVersionFromRef -RepoRoot $script:Ws.Path -BaseRef 'HEAD' -PackageFolder 'b' | Should -Be '0.2.1'
    }

    It 'reads the older version from a prior commit' {
        Get-PackageVersionFromRef -RepoRoot $script:Ws.Path -BaseRef 'HEAD~1' -PackageFolder 'b' | Should -Be '0.2.0'
    }

    It 'returns null when the package does not exist at the ref' {
        Get-PackageVersionFromRef -RepoRoot $script:Ws.Path -BaseRef 'HEAD' -PackageFolder 'nonexistent' | Should -BeNullOrEmpty
    }
}

Describe 'Get-PackageLastReleaseBaseline' {
    BeforeAll {
        Reset-ReleaseScriptCaches
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'baseline')
        # Initial commit (call it C0). Now make a source-only edit (C1), then a
        # version-changing commit (C2), then another source edit (C3). Baseline
        # for 'b' must be C2.
        $script:Ws.ModifySource('b')
        $script:Ws.AddCommit('source edit to b')
        $script:Sha_C1 = $script:Ws.GitSha('HEAD')

        $script:Ws.SetVersion('b', '0.2.1')
        $script:Ws.AddCommit('change b to 0.2.1')
        $script:Sha_C2 = $script:Ws.GitSha('HEAD')

        $script:Ws.ModifySource('b')
        $script:Ws.AddCommit('further source edit to b')
        $script:Sha_C3 = $script:Ws.GitSha('HEAD')
    }

    It 'returns the SHA of the most recent version-changing commit' {
        $sha = Get-PackageLastReleaseBaseline -RepoRoot $script:Ws.Path -PackageFolder 'b'
        $sha | Should -Be $script:Sha_C2
    }

    It 'returns null for a package folder that has never existed' {
        Get-PackageLastReleaseBaseline -RepoRoot $script:Ws.Path -PackageFolder 'nonexistent' | Should -BeNullOrEmpty
    }
}

Describe 'Get-WorkspacePackages' {
    BeforeAll {
        Reset-ReleaseScriptCaches
    $script:Ws = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'wscrates')
    }

    It 'returns one entry per workspace package' {
        $packages = Get-WorkspacePackages -repoRoot $script:Ws.Path
        $packages.Count | Should -Be 6
    }

    It 'reports Published=$false for publish=false packages' {
        $packages = Get-WorkspacePackages -repoRoot $script:Ws.Path
        $util = $packages | Where-Object { $_.Name -eq 'utility' }
        $util.Published | Should -BeFalse
        ($packages | Where-Object { $_.Name -ne 'utility' } | ForEach-Object Published) | ForEach-Object { $_ | Should -BeTrue }
    }

    It 'excludes dev-deps from Deps' {
        $packages = Get-WorkspacePackages -repoRoot $script:Ws.Path
        $target = $packages | Where-Object { $_.Name -eq 'target' }
        # Mixed6 wires a normal dep on upstream_b and a dev dep on upstream_a;
        # Get-WorkspacePackages flattens to normal/build only.
        $target.Deps | Should -Contain 'upstream_b'
        $target.Deps | Should -Not -Contain 'upstream_a'
    }
}

Describe 'Get-AllTransitiveDependents' {
    BeforeAll {
        Reset-ReleaseScriptCaches
    $script:Ws = New-SyntheticWorkspace -Preset Diamond4 -Path (Join-Path $TestDrive 'transitive-diamond')
        # Diamond4: top -> {left, right}; left -> bottom; right -> bottom. Dependents of bottom are {left, right, top}.
    }

    It 'finds dependents through both diamond legs (deduped)' {
        $deps = Get-AllTransitiveDependents -packageName 'bottom' -repoRoot $script:Ws.Path
        ($deps | Sort-Object) | Should -Be @('left', 'right', 'top')
    }

    It 'returns no dependents for a leaf (top) package' {
        # 'top' is the top of the diamond; nothing depends on it.
        $deps = @(Get-AllTransitiveDependents -packageName 'top' -repoRoot $script:Ws.Path)
        $deps.Count | Should -Be 0
    }

    It 'excludes publish=false packages from the result' {
        # Use Mixed6 — utility is publish=false and depends on downstream_y.
        Reset-ReleaseScriptCaches
        $mixed = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'transitive-mixed')
        $deps = Get-AllTransitiveDependents -packageName 'downstream_y' -repoRoot $mixed.Path
        # utility depends on downstream_y but is publish=false; should not appear.
        $deps | Should -Not -Contain 'utility'
    }
}

Describe 'Get-PackagesWithUnreleasedChanges' {
    BeforeAll {
        Reset-ReleaseScriptCaches
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleasedchanges')
        # Edit package b after initial commit and commit it.
        $script:Ws.ModifySource('b')
        $script:Ws.AddCommit('b edit')
    }

    It 'reports committed source edits as unreleased' {
        $changes = Get-PackagesWithUnreleasedChanges -RepoRoot $script:Ws.Path
        $changes.ContainsKey('b') | Should -BeTrue
        $changes['b'] | Should -BeGreaterOrEqual 1
    }

    It 'reports working-tree edits as unreleased' {
        Reset-ReleaseScriptCaches
        $w2 = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleasedworking')
        # Uncommitted source edit on c.
        $w2.ModifySource('c')
        $changes = Get-PackagesWithUnreleasedChanges -RepoRoot $w2.Path
        $changes.ContainsKey('c') | Should -BeTrue
    }

    It 'reports untracked files as unreleased' {
        Reset-ReleaseScriptCaches
        $w3 = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleaseduntracked')
        $newFile = Join-Path $w3.Path 'crates\a\src\new_file.rs'
        Set-Content -Path $newFile -Value '// new'
        $changes = Get-PackagesWithUnreleasedChanges -RepoRoot $w3.Path
        $changes.ContainsKey('a') | Should -BeTrue
    }

    It 'skips publish=false packages' {
        Reset-ReleaseScriptCaches
        $w4 = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'unreleasedmixed')
        $w4.ModifySource('utility')
        $w4.AddCommit('utility edit')
        $changes = Get-PackagesWithUnreleasedChanges -RepoRoot $w4.Path
        $changes.ContainsKey('utility') | Should -BeFalse
    }
}

Describe 'Get-PackagesWithVersionChanges' {
    BeforeAll {
        Reset-ReleaseScriptCaches
    $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'versionchanges')
        # Change version of 'a' relative to HEAD~ baseline.
        $script:Ws.SetVersion('a', '0.1.1')
        $script:Ws.AddCommit('change a')
    }

    It 'reports a package whose version differs vs the base ref' {
        $changedSet = Get-PackagesWithVersionChanges -RepoRoot $script:Ws.Path -BaseRef 'HEAD~1'
        $changedSet.Contains('a') | Should -BeTrue
        $changedSet.Contains('b') | Should -BeFalse
        $changedSet.Contains('c') | Should -BeFalse
    }

    It 'returns an empty set when the working tree matches the base ref' {
        Reset-ReleaseScriptCaches
        $w2 = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'versionchanges-empty')
        $changedSet = Get-PackagesWithVersionChanges -RepoRoot $w2.Path -BaseRef 'HEAD'
        $changedSet.Count | Should -Be 0
    }
}

Describe 'Get-PendingReleases' {
    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    It 'returns an empty array when no version diffs against the base ref' {
        $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive ('pend-empty-' + [guid]::NewGuid().Guid.Substring(0,8)))
        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef 'HEAD')
        $pending.Count | Should -Be 0
    }

    It 'reports one record per pending package with Folder/Name/BaseVersion/CurrentVersion' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-single-' + [guid]::NewGuid().Guid.Substring(0,8)))
        # Change version of 'b' on top of HEAD baseline (uncommitted — that's the "pending" state).
        $ws.SetVersion('b', '0.2.2')

        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef 'HEAD')
        $pending.Count             | Should -Be 1
        $pending[0].Folder         | Should -Be 'b'
        $pending[0].Name           | Should -Be 'b'
        $pending[0].BaseVersion    | Should -Be '0.2.0'
        $pending[0].CurrentVersion | Should -Be '0.2.2'
    }

    It 'sorts pending records by Folder ascending so the announcement is deterministic' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-sort-' + [guid]::NewGuid().Guid.Substring(0,8)))
        # Change versions in reverse Folder order — the helper must still emit them in alphabetical order.
        $ws.SetVersion('c', '0.3.1')
        $ws.SetVersion('a', '0.1.1')
        $ws.SetVersion('b', '0.2.1')

        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef 'HEAD')
        ($pending | ForEach-Object { $_.Folder }) -join ',' | Should -Be 'a,b,c'
    }

    It 'skips new-at-base packages (no base version to compare against)' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-new-' + [guid]::NewGuid().Guid.Substring(0,8)))
        # Change version of existing 'a' so we have at least one genuinely-pending entry to compare against.
        $ws.SetVersion('a', '0.1.1')

        # Manually scaffold a brand-new package that doesn't exist at HEAD (no add+commit).
        $newPackage = Join-Path $ws.Path 'crates\brandnew'
        New-Item -ItemType Directory -Path $newPackage -Force | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $newPackage 'src') -Force | Out-Null
        Set-Content -Path (Join-Path $newPackage 'Cargo.toml') -Value @"
[package]
name = "brandnew"
version = "0.1.0"
edition = "2021"
"@ -NoNewline
        Set-Content -Path (Join-Path $newPackage 'src\lib.rs') -Value '' -NoNewline

        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef 'HEAD')
        ($pending | ForEach-Object { $_.Folder }) | Should -Be @('a')
    }

    It 'returns an empty array when BaseRef is empty (no base to compare against)' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-norefs-' + [guid]::NewGuid().Guid.Substring(0,8)))
        $ws.SetVersion('a', '0.1.1')

        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef '')
        $pending.Count | Should -Be 0
    }
}

Describe 'Get-FileLineEnding' {
    It 'returns LF for a file with only LF endings' {
        $path = Join-Path $TestDrive ('eol-lf-' + [guid]::NewGuid().Guid.Substring(0,8) + '.txt')
        [System.IO.File]::WriteAllText($path, "alpha`nbeta`ngamma`n")
        Get-FileLineEnding -Path $path | Should -Be "`n"
    }

    It 'returns CRLF for a file with only CRLF endings' {
        $path = Join-Path $TestDrive ('eol-crlf-' + [guid]::NewGuid().Guid.Substring(0,8) + '.txt')
        [System.IO.File]::WriteAllText($path, "alpha`r`nbeta`r`ngamma`r`n")
        Get-FileLineEnding -Path $path | Should -Be "`r`n"
    }

    It 'returns the dominant style for a mixed file' {
        $path = Join-Path $TestDrive ('eol-mixed-' + [guid]::NewGuid().Guid.Substring(0,8) + '.txt')
        # 3 CRLFs vs 1 lone LF
        [System.IO.File]::WriteAllText($path, "a`r`nb`r`nc`r`nd`ne")
        Get-FileLineEnding -Path $path | Should -Be "`r`n"
    }

    It 'returns LF as the default for a missing file' {
        Get-FileLineEnding -Path (Join-Path $TestDrive 'no-such-file.txt') | Should -Be "`n"
    }

    It 'returns LF as the default for an empty file' {
        $path = Join-Path $TestDrive ('eol-empty-' + [guid]::NewGuid().Guid.Substring(0,8) + '.txt')
        [System.IO.File]::WriteAllText($path, '')
        Get-FileLineEnding -Path $path | Should -Be "`n"
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
        $content | Should -Match 'Now requires `0\.3\.0` of `depcrate`'
        $content | Should -Not -Match '- ⚠️ Breaking'
    }

    It 'inserts a Breaking section bullet when the cascade is breaking' {
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'bigdep'; Version = '1.0.0'; Breaking = $true
        }
        $content = Get-Content $script:Changelog -Raw
        $content | Should -Match '- ⚠️ Breaking'
        $content | Should -Match 'Now requires `1\.0\.0` of `bigdep`'
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

    It 'preserves LF line endings when the source file uses LF' {
        $path = Join-Path $TestDrive ('cascade-lf-' + [guid]::NewGuid().Guid.Substring(0,8) + '.md')
        [System.IO.File]::WriteAllText($path, "# Changelog`n`n## [0.2.0] - 2025-01-01`n`n- ✨ Features`n`n  - already noted`n`n")
        Add-CascadeBulletToVersionSection -ChangelogFile $path -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.3.0'; Breaking = $false
        }
        $raw = [System.IO.File]::ReadAllText($path)
        ([regex]::Matches($raw, "`r`n")).Count | Should -Be 0
        ([regex]::Matches($raw, "(?<!`r)`n")).Count | Should -BeGreaterThan 0
        $raw | Should -Match 'Now requires `0\.3\.0` of `depcrate`'
    }

    It 'preserves CRLF line endings when the source file uses CRLF' {
        $path = Join-Path $TestDrive ('cascade-crlf-' + [guid]::NewGuid().Guid.Substring(0,8) + '.md')
        [System.IO.File]::WriteAllText($path, "# Changelog`r`n`r`n## [0.2.0] - 2025-01-01`r`n`r`n- ✨ Features`r`n`r`n  - already noted`r`n`r`n")
        Add-CascadeBulletToVersionSection -ChangelogFile $path -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.3.0'; Breaking = $false
        }
        $raw = [System.IO.File]::ReadAllText($path)
        ([regex]::Matches($raw, "(?<!`r)`n")).Count | Should -Be 0
        ([regex]::Matches($raw, "`r`n")).Count | Should -BeGreaterThan 0
        $raw | Should -Match 'Now requires `0\.3\.0` of `depcrate`'
    }

    # --- dedup behavior: escalations re-fire the cascade with a HIGHER target
    # version (and sometimes a different breaking/maintenance classification).
    # Without the dedup, the section would accumulate stale bullets citing the
    # earlier target version. These tests pin the "drop pre-existing bullets
    # for the same target before inserting the new one" behavior.

    It 'drops a pre-existing bullet for the same target before inserting the escalated one' {
        # First cascade lands at 0.2.0; later the upstream is escalated to 0.3.0
        # and we re-fire the cascade. The 0.2.0 bullet must be removed so the
        # section only cites the final required version.
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.2.0'; Breaking = $false
        }
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.3.0'; Breaking = $false
        }
        $content = Get-Content $script:Changelog -Raw
        $content | Should -Match 'Now requires `0\.3\.0` of `depcrate`'
        $content | Should -Not -Match 'Now requires `0\.2\.0` of `depcrate`'
        # Exactly one bullet remains for this target.
        ([regex]::Matches($content, 'Now requires `[^`]+` of `depcrate`')).Count | Should -Be 1
    }

    It 'drops a stale Maintenance bullet when escalating to a breaking cascade for the same target' {
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.2.5'; Breaking = $false
        }
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '1.0.0'; Breaking = $true
        }
        $content = Get-Content $script:Changelog -Raw
        # New Breaking-section bullet present, old Maintenance bullet gone.
        $content | Should -Match '- ⚠️ Breaking'
        $content | Should -Match 'Now requires `1\.0\.0` of `depcrate`'
        $content | Should -Not -Match 'Now requires `0\.2\.5` of `depcrate`'
    }

    It 'leaves bullets for unrelated targets untouched when escalating' {
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'other'; Version = '0.1.0'; Breaking = $false
        }
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.2.0'; Breaking = $false
        }
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.3.0'; Breaking = $false
        }
        $content = Get-Content $script:Changelog -Raw
        $content | Should -Match 'Now requires `0\.1\.0` of `other`'
        $content | Should -Match 'Now requires `0\.3\.0` of `depcrate`'
        $content | Should -Not -Match 'Now requires `0\.2\.0` of `depcrate`'
    }

    It 'leaves bullets in OTHER version sections untouched when escalating in this section' {
        # Two pending sections — the dedup must be scoped to the section
        # being edited so prior-release bullets in [0.1.0] are not clobbered.
        Set-Content -Path $script:Changelog -Value @'
# Changelog

## [0.2.0] - 2025-01-01

- 🔧 Maintenance

  - Now requires `0.2.0` of `depcrate`

## [0.1.0] - 2024-01-01

- 🔧 Maintenance

  - Now requires `0.1.5` of `depcrate`

'@
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.3.0'; Breaking = $false
        }
        $content = Get-Content $script:Changelog -Raw
        # The 0.2.0 section was rewritten — old 0.2.0 cite gone, 0.3.0 cite present.
        $content | Should -Match '## \[0\.2\.0\][\s\S]*Now requires `0\.3\.0` of `depcrate`'
        # The 0.1.0 section's historical bullet is preserved verbatim.
        $content | Should -Match '## \[0\.1\.0\][\s\S]*Now requires `0\.1\.5` of `depcrate`'
    }

    It 'leaves a bullet for a target whose name is a strict suffix/prefix of TargetName untouched' {
        # Bullet anchoring: `depcrate-extra` must not match a dedup pattern
        # built for `depcrate`, and vice-versa.
        Set-Content -Path $script:Changelog -Value @'
# Changelog

## [0.2.0] - 2025-01-01

- 🔧 Maintenance

  - Now requires `0.1.0` of `depcrate-extra`
  - Now requires `0.1.0` of `xdepcrate`
  - Now requires `0.2.0` of `depcrate`

'@
        Add-CascadeBulletToVersionSection -ChangelogFile $script:Changelog -Version '0.2.0' -CascadeReason @{
            Target = 'depcrate'; Version = '0.3.0'; Breaking = $false
        }
        $content = Get-Content $script:Changelog -Raw
        $content | Should -Match 'Now requires `0\.1\.0` of `depcrate-extra`'
        $content | Should -Match 'Now requires `0\.1\.0` of `xdepcrate`'
        $content | Should -Match 'Now requires `0\.3\.0` of `depcrate`'
        $content | Should -Not -Match 'Now requires `0\.2\.0` of `depcrate`'
    }
}

# ---------------------------------------------------------------------------
# Update-PendingReleaseVersion — in-place re-stamp of a partially-released
# pending release. Used by Invoke-CascadeStep when an upstream cascade
# escalates a downstream that's already been version-changed in this PR: we must
# rewrite the Cargo.toml + workspace dep + CHANGELOG header in place to
# avoid leaving two stale `## [old]` / `## [new]` sections side by side.
# ---------------------------------------------------------------------------

Describe 'Update-PendingReleaseVersion' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    BeforeEach {
        Reset-ReleaseScriptCaches
        $script:UprvWs = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive ('uprv-' + [guid]::NewGuid().Guid.Substring(0,8)))
        # Pre-release downstream to 0.1.1 so there's a pending release for us to
        # re-stamp. Also seed a CHANGELOG with the matching pending section so
        # we exercise the header-rewrite branch.
        $script:UprvPackageFolder = Join-Path $script:UprvWs.Path 'crates\downstream'
        $script:UprvCargoToml   = Join-Path $script:UprvPackageFolder 'Cargo.toml'
        $script:UprvChangelog   = Join-Path $script:UprvPackageFolder 'CHANGELOG.md'
        $script:UprvRootCargo   = Join-Path $script:UprvWs.Path 'Cargo.toml'

        $script:UprvWs.SetVersion('downstream', '0.1.1')
        # Manually re-stamp the workspace entry too so the fixture matches what
        # release-crate.ps1 would have written when first releasing downstream.
        $rootContent = Get-Content $script:UprvRootCargo -Raw
        $rootContent = $rootContent -replace '(downstream\s*=\s*\{[^}]*?version\s*=\s*")[^"]+', '${1}0.1.1'
        Set-Content -Path $script:UprvRootCargo -Value $rootContent -NoNewline

        Set-Content -Path $script:UprvChangelog -Value @"
# Changelog

## [0.1.1] - 2025-01-01

- ✨ Features

  - first feature

"@
    }

    It 'rewrites the [package].version in the package Cargo.toml' {
        Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0' | Out-Null
        $content = Get-Content $script:UprvCargoToml -Raw
        $content | Should -Match '(?m)^[ \t]*version\s*=\s*"0\.2\.0"'
        $content | Should -Not -Match '(?m)^[ \t]*version\s*=\s*"0\.1\.1"'
    }

    It 'rewrites the [workspace.dependencies] entry for the package in the root Cargo.toml' {
        Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0' | Out-Null
        $root = Get-Content $script:UprvRootCargo -Raw
        $root | Should -Match 'downstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.0"'
        # And no stale 0.1.1 reference for downstream in the root file.
        $root | Should -Not -Match 'downstream\s*=\s*\{[^}]*version\s*=\s*"0\.1\.1"'
    }

    It 'leaves OTHER workspace dep entries untouched' {
        # Change downstream's pending version; upstream's workspace entry must
        # remain at its declared 0.2.0.
        Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0' | Out-Null
        $root = Get-Content $script:UprvRootCargo -Raw
        $root | Should -Match 'upstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.0"'
    }

    It 'rewrites the matching CHANGELOG section header from [old] to [new]' {
        Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0' | Out-Null
        $log = Get-Content $script:UprvChangelog -Raw
        $log | Should -Match '## \[0\.2\.0\]'
        $log | Should -Not -Match '## \[0\.1\.1\]'
        # Body of the section is preserved verbatim.
        $log | Should -Match 'first feature'
    }

    It 'is a no-op when OldVersion equals NewVersion' {
        $before = Get-Content $script:UprvCargoToml -Raw
        $beforeRoot = Get-Content $script:UprvRootCargo -Raw
        $beforeLog  = Get-Content $script:UprvChangelog -Raw
        Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.1.1' | Out-Null
        (Get-Content $script:UprvCargoToml -Raw) | Should -Be $before
        (Get-Content $script:UprvRootCargo -Raw) | Should -Be $beforeRoot
        (Get-Content $script:UprvChangelog -Raw) | Should -Be $beforeLog
    }

    It 'still rewrites Cargo.toml + root when the changelog is missing (warning emitted)' {
        Remove-Item -LiteralPath $script:UprvChangelog -Force
        $warnings = @()
        Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0' `
            -WarningVariable warnings -WarningAction SilentlyContinue | Out-Null
        (Get-Content $script:UprvCargoToml -Raw) | Should -Match '(?m)^[ \t]*version\s*=\s*"0\.2\.0"'
        (Get-Content $script:UprvRootCargo -Raw) | Should -Match 'downstream\s*=\s*\{[^}]*version\s*=\s*"0\.2\.0"'
        $warnings.Count | Should -BeGreaterOrEqual 1
        ($warnings -join ' ') | Should -Match 'not found'
    }

    It 'warns when the changelog has no `## [OldVersion]` section to rewrite' {
        Set-Content -Path $script:UprvChangelog -Value @"
# Changelog

## [9.9.9] - 2030-01-01

- ✨ Features

  - mismatched

"@
        $warnings = @()
        Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0' `
            -WarningVariable warnings -WarningAction SilentlyContinue | Out-Null
        $warnings.Count | Should -BeGreaterOrEqual 1
        ($warnings -join ' ') | Should -Match '0\.1\.1'
        # Cargo.toml + root still rewritten — the changelog miss is a non-fatal warning.
        (Get-Content $script:UprvCargoToml -Raw) | Should -Match 'version\s*=\s*"0\.2\.0"'
    }

    It 'throws when the package Cargo.toml does not exist' {
        $bogusFolder = Join-Path $script:UprvWs.Path 'crates\nope'
        {
            Update-PendingReleaseVersion -PackageName 'nope' -PackageFolder $bogusFolder `
                -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0'
        } | Should -Throw
    }

    It 'returns the NewVersion string' {
        $r = Update-PendingReleaseVersion -PackageName 'downstream' -PackageFolder $script:UprvPackageFolder `
            -RootCargoToml $script:UprvRootCargo -OldVersion '0.1.1' -NewVersion '0.2.0'
        $r | Should -Be '0.2.0'
    }
}

# ---------------------------------------------------------------------------
# Session-scoped caches: ensures the analyze-phase optimizations (warm git
# lookups across post-release dep-scan iterations) actually take effect, and
# that test isolation via Reset-ReleaseScriptCaches still clears them.
# ---------------------------------------------------------------------------

Describe 'Session-scoped git caches' {
    BeforeAll {
        $script:CacheWs = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'cache-tests')
        # Build a 3-commit history so the per-package baseline and committed
        # diff are both well-defined and distinct from HEAD:
        #   HEAD~2 → initial Linear3 (b at 0.2.0)
        #   HEAD~1 → version change for b to 0.2.1 (this is the baseline commit)
        #   HEAD   → source edit for b (an unreleased change visible to
        #            Get-PackageCommittedChanges)
        $script:CacheWs.SetVersion('b', '0.2.1')
        $script:CacheWs.AddCommit('change b')
        $script:CacheWs.ModifySource('b', '// post-baseline edit')
        $script:CacheWs.AddCommit('edit b after baseline')
    }

    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    Context 'Get-PackageVersionFromRef' {
        It 'returns the cached value on the second call (no git spawn)' {
            $v1 = Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'b'
            $v1 | Should -Be '0.2.1'
            # If the cache is bypassed, Invoke-Git will be called and the mock
            # below will throw. A passing test confirms the cache served the
            # second request without touching git.
            Mock -CommandName Invoke-Git -MockWith { throw "Invoke-Git called when cache should have served the request" }
            $v2 = Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'b'
            $v2 | Should -Be '0.2.1'
        }

        It 'caches null results for nonexistent package folders' {
            (Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'nope') | Should -BeNullOrEmpty
            Mock -CommandName Invoke-Git -MockWith { throw "second call should be cached" }
            (Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'nope') | Should -BeNullOrEmpty
        }

        It 'uses different cache slots for different BaseRefs of the same folder' {
            # Populate cache for HEAD only.
            (Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'b') | Should -Be '0.2.1'
            # HEAD~2 must still hit git (different cache key) and return the original version
            # (HEAD~1 is the version-change commit, also at 0.2.1; HEAD~2 is the initial Linear3 state).
            (Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD~2' -PackageFolder 'b') | Should -Be '0.2.0'
        }
    }

    Context 'Get-PackageLastReleaseBaseline' {
        It 'returns the cached SHA on the second call (no git spawn)' {
            $sha1 = Get-PackageLastReleaseBaseline -RepoRoot $script:CacheWs.Path -PackageFolder 'b'
            $sha1 | Should -Not -BeNullOrEmpty
            Mock -CommandName Invoke-Git -MockWith { throw "Invoke-Git called when cache should have served the request" }
            $sha2 = Get-PackageLastReleaseBaseline -RepoRoot $script:CacheWs.Path -PackageFolder 'b'
            $sha2 | Should -Be $sha1
        }
    }

    Context 'Get-PackageCommittedChanges' {
        It 'returns the same array on the second call (no git spawn)' {
            $files1 = Get-PackageCommittedChanges -RepoRoot $script:CacheWs.Path -PackageFolder 'b'
            # 'b' has had its version changed after its baseline (Cargo.toml + maybe changelog),
            # but at minimum the Cargo.toml change must show up.
            $files1.Count | Should -BeGreaterOrEqual 1
            Mock -CommandName Invoke-Git -MockWith { throw "Invoke-Git called when cache should have served the request" }
            $files2 = Get-PackageCommittedChanges -RepoRoot $script:CacheWs.Path -PackageFolder 'b'
            $files2.Count | Should -Be $files1.Count
        }

        It 'returns an empty result for a package with no prior baseline' {
            # The 'nope' folder doesn't exist, so Get-PackageLastReleaseBaseline returns $null.
            # PowerShell idiomatically unwraps empty arrays to $null at the function
            # boundary; consumers should collect with @(...) when array shape matters.
            $files = @(Get-PackageCommittedChanges -RepoRoot $script:CacheWs.Path -PackageFolder 'nope')
            $files.Count | Should -Be 0
        }
    }

    Context 'Reset-ReleaseScriptCaches' {
        It 'clears every session-scoped cache so the next call re-fetches' {
            # Prime all three caches.
            (Get-PackageVersionFromRef       -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'b') | Out-Null
            (Get-PackageLastReleaseBaseline  -RepoRoot $script:CacheWs.Path -PackageFolder 'b')                 | Out-Null
            (Get-PackageCommittedChanges     -RepoRoot $script:CacheWs.Path -PackageFolder 'b')                 | Out-Null

            Reset-ReleaseScriptCaches

            # After reset, the next call MUST hit git. We assert this by mocking
            # Invoke-Git to record a call — if reset failed, the cache would
            # serve the call and the mock counter stays at 0.
            Mock -CommandName Invoke-Git -MockWith { @() }
            (Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'b') | Out-Null
            Should -Invoke -CommandName Invoke-Git -Times 1 -Exactly
        }
    }

    Context 'Invalidate-WorkspaceMetadataCache' {
        It 'does NOT clear git-derived caches (production cascade calls must not undo the speed-up)' {
            $v1 = Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'b'
            Invalidate-WorkspaceMetadataCache
            Mock -CommandName Invoke-Git -MockWith { throw "Invalidate-WorkspaceMetadataCache must leave git caches intact" }
            $v2 = Get-PackageVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -PackageFolder 'b'
            $v2 | Should -Be $v1
        }
    }
}
