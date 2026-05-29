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
        Reset-ReleaseScriptCaches
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
        Reset-ReleaseScriptCaches
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
        Reset-ReleaseScriptCaches
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
        Reset-ReleaseScriptCaches
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
        Reset-ReleaseScriptCaches
        $mixed = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'transitive-mixed')
        $deps = Get-AllTransitiveDependents -crateName 'downstream_y' -repoRoot $mixed.Path
        # utility depends on downstream_y but is publish=false; should not appear.
        $deps | Should -Not -Contain 'utility'
    }
}

Describe 'Get-CratesWithUnreleasedChanges' {
    BeforeAll {
        Reset-ReleaseScriptCaches
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
        Reset-ReleaseScriptCaches
        $w2 = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleasedworking')
        # Uncommitted source edit on c.
        $w2.ModifySource('c')
        $changes = Get-CratesWithUnreleasedChanges -RepoRoot $w2.Path
        $changes.ContainsKey('c') | Should -BeTrue
    }

    It 'reports untracked files as unreleased' {
        Reset-ReleaseScriptCaches
        $w3 = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'unreleaseduntracked')
        $newFile = Join-Path $w3.Path 'crates\a\src\new_file.rs'
        Set-Content -Path $newFile -Value '// new'
        $changes = Get-CratesWithUnreleasedChanges -RepoRoot $w3.Path
        $changes.ContainsKey('a') | Should -BeTrue
    }

    It 'skips publish=false crates' {
        Reset-ReleaseScriptCaches
        $w4 = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'unreleasedmixed')
        $w4.ModifySource('utility')
        $w4.AddCommit('utility edit')
        $changes = Get-CratesWithUnreleasedChanges -RepoRoot $w4.Path
        $changes.ContainsKey('utility') | Should -BeFalse
    }
}

Describe 'Get-CratesWithVersionBumps' {
    BeforeAll {
        Reset-ReleaseScriptCaches
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
        Reset-ReleaseScriptCaches
        $w2 = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive 'versionbumps-empty')
        $bumped = Get-CratesWithVersionBumps -RepoRoot $w2.Path -BaseRef 'HEAD'
        $bumped.Count | Should -Be 0
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
        # Bump 'b' on top of HEAD baseline (uncommitted — that's the "pending" state).
        $ws.BumpVersion('b', '0.2.2')

        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef 'HEAD')
        $pending.Count             | Should -Be 1
        $pending[0].Folder         | Should -Be 'b'
        $pending[0].Name           | Should -Be 'b'
        $pending[0].BaseVersion    | Should -Be '0.2.0'
        $pending[0].CurrentVersion | Should -Be '0.2.2'
    }

    It 'sorts pending records by Folder ascending so the announcement is deterministic' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-sort-' + [guid]::NewGuid().Guid.Substring(0,8)))
        # Bump in reverse Folder order — the helper must still emit them in alphabetical order.
        $ws.BumpVersion('c', '0.3.1')
        $ws.BumpVersion('a', '0.1.1')
        $ws.BumpVersion('b', '0.2.1')

        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef 'HEAD')
        ($pending | ForEach-Object { $_.Folder }) -join ',' | Should -Be 'a,b,c'
    }

    It 'skips new-at-base crates (no base version to compare against)' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-new-' + [guid]::NewGuid().Guid.Substring(0,8)))
        # Bump existing 'a' so we have at least one genuinely-pending entry to compare against.
        $ws.BumpVersion('a', '0.1.1')

        # Manually scaffold a brand-new crate that doesn't exist at HEAD (no add+commit).
        $newCrate = Join-Path $ws.Path 'crates\brandnew'
        New-Item -ItemType Directory -Path $newCrate -Force | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $newCrate 'src') -Force | Out-Null
        Set-Content -Path (Join-Path $newCrate 'Cargo.toml') -Value @"
[package]
name = "brandnew"
version = "0.1.0"
edition = "2021"
"@ -NoNewline
        Set-Content -Path (Join-Path $newCrate 'src\lib.rs') -Value '' -NoNewline

        $pending = @(Get-PendingReleases -RepoRoot $ws.Path -BaseRef 'HEAD')
        ($pending | ForEach-Object { $_.Folder }) | Should -Be @('a')
    }

    It 'returns an empty array when BaseRef is empty (no base to compare against)' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-norefs-' + [guid]::NewGuid().Guid.Substring(0,8)))
        $ws.BumpVersion('a', '0.1.1')

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
}

# ---------------------------------------------------------------------------
# Session-scoped caches: ensures the analyze-phase optimizations (warm git
# lookups across post-release dep-scan iterations) actually take effect, and
# that test isolation via Reset-ReleaseScriptCaches still clears them.
# ---------------------------------------------------------------------------

Describe 'Session-scoped git caches' {
    BeforeAll {
        $script:CacheWs = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'cache-tests')
        # Build a 3-commit history so the per-crate baseline and committed
        # diff are both well-defined and distinct from HEAD:
        #   HEAD~2 → initial Linear3 (b at 0.2.0)
        #   HEAD~1 → version bump for b to 0.2.1 (this is the baseline commit)
        #   HEAD   → source edit for b (an unreleased change visible to
        #            Get-CrateCommittedChanges)
        $script:CacheWs.BumpVersion('b', '0.2.1')
        $script:CacheWs.AddCommit('bump b')
        $script:CacheWs.ModifySource('b', '// post-baseline edit')
        $script:CacheWs.AddCommit('edit b after baseline')
    }

    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    Context 'Get-CrateVersionFromRef' {
        It 'returns the cached value on the second call (no git spawn)' {
            $v1 = Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'b'
            $v1 | Should -Be '0.2.1'
            # If the cache is bypassed, Invoke-Git will be called and the mock
            # below will throw. A passing test confirms the cache served the
            # second request without touching git.
            Mock -CommandName Invoke-Git -MockWith { throw "Invoke-Git called when cache should have served the request" }
            $v2 = Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'b'
            $v2 | Should -Be '0.2.1'
        }

        It 'caches null results for nonexistent crate folders' {
            (Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'nope') | Should -BeNullOrEmpty
            Mock -CommandName Invoke-Git -MockWith { throw "second call should be cached" }
            (Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'nope') | Should -BeNullOrEmpty
        }

        It 'uses different cache slots for different BaseRefs of the same folder' {
            # Populate cache for HEAD only.
            (Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'b') | Should -Be '0.2.1'
            # HEAD~2 must still hit git (different cache key) and return the original version
            # (HEAD~1 is the bump commit, also at 0.2.1; HEAD~2 is the initial Linear3 state).
            (Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD~2' -CrateFolder 'b') | Should -Be '0.2.0'
        }
    }

    Context 'Get-CrateLastReleaseBaseline' {
        It 'returns the cached SHA on the second call (no git spawn)' {
            $sha1 = Get-CrateLastReleaseBaseline -RepoRoot $script:CacheWs.Path -CrateFolder 'b'
            $sha1 | Should -Not -BeNullOrEmpty
            Mock -CommandName Invoke-Git -MockWith { throw "Invoke-Git called when cache should have served the request" }
            $sha2 = Get-CrateLastReleaseBaseline -RepoRoot $script:CacheWs.Path -CrateFolder 'b'
            $sha2 | Should -Be $sha1
        }
    }

    Context 'Get-CrateCommittedChanges' {
        It 'returns the same array on the second call (no git spawn)' {
            $files1 = Get-CrateCommittedChanges -RepoRoot $script:CacheWs.Path -CrateFolder 'b'
            # 'b' has had its version bumped after its baseline (Cargo.toml + maybe changelog),
            # but at minimum the Cargo.toml change must show up.
            $files1.Count | Should -BeGreaterOrEqual 1
            Mock -CommandName Invoke-Git -MockWith { throw "Invoke-Git called when cache should have served the request" }
            $files2 = Get-CrateCommittedChanges -RepoRoot $script:CacheWs.Path -CrateFolder 'b'
            $files2.Count | Should -Be $files1.Count
        }

        It 'returns an empty result for a crate with no prior baseline' {
            # The 'nope' folder doesn't exist, so Get-CrateLastReleaseBaseline returns $null.
            # PowerShell idiomatically unwraps empty arrays to $null at the function
            # boundary; consumers should collect with @(...) when array shape matters.
            $files = @(Get-CrateCommittedChanges -RepoRoot $script:CacheWs.Path -CrateFolder 'nope')
            $files.Count | Should -Be 0
        }
    }

    Context 'Reset-ReleaseScriptCaches' {
        It 'clears every session-scoped cache so the next call re-fetches' {
            # Prime all three caches.
            (Get-CrateVersionFromRef       -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'b') | Out-Null
            (Get-CrateLastReleaseBaseline  -RepoRoot $script:CacheWs.Path -CrateFolder 'b')                 | Out-Null
            (Get-CrateCommittedChanges     -RepoRoot $script:CacheWs.Path -CrateFolder 'b')                 | Out-Null

            Reset-ReleaseScriptCaches

            # After reset, the next call MUST hit git. We assert this by mocking
            # Invoke-Git to record a call — if reset failed, the cache would
            # serve the call and the mock counter stays at 0.
            Mock -CommandName Invoke-Git -MockWith { @() }
            (Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'b') | Out-Null
            Should -Invoke -CommandName Invoke-Git -Times 1 -Exactly
        }
    }

    Context 'Invalidate-WorkspaceMetadataCache' {
        It 'does NOT clear git-derived caches (production cascade calls must not undo the speed-up)' {
            $v1 = Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'b'
            Invalidate-WorkspaceMetadataCache
            Mock -CommandName Invoke-Git -MockWith { throw "Invalidate-WorkspaceMetadataCache must leave git caches intact" }
            $v2 = Get-CrateVersionFromRef -RepoRoot $script:CacheWs.Path -BaseRef 'HEAD' -CrateFolder 'b'
            $v2 | Should -Be $v1
        }
    }
}
