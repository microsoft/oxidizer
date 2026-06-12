# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Phase 4 — git / filesystem unit tests. Each test builds a tiny synthetic
# workspace via New-SyntheticWorkspace and exercises one helper from
# scripts/lib/releasing.ps1 in isolation. The workspace metadata cache is
# invalidated between tests so a previous fixture cannot pollute the next.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
    . (Join-Path $PSScriptRoot '..\..\_common\New-SyntheticWorkspace.ps1')
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
        $cargo = Join-Path $script:Ws.Path 'crates\dependency\Cargo.toml'
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

Describe 'Get-PackageLastReleaseBaseline (TOML publish-syntax variants)' {

    # The baseline detector inspects committed history for diff hunks whose
    # added/removed lines start with `version = ...`, `publish = ...`,
    # `version.workspace = ...`, or `publish.workspace = ...`. The dotted-key
    # forms are TOML-equivalent to the inline-table forms but tools commonly
    # round-trip between them; the regex must accept both.

    It 'recognises a commit that flips publish = false → publish.workspace = true as a baseline reset' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'pkg'; Version = '0.1.0'; PublishSyntax = 'literal-false' }
            )
            WorkspacePackage = [ordered]@{ publish = 'true' }
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'pubflip-literal-to-dotted')

        # Source edit before the flip so we have a non-baseline commit between
        # C0 (initial) and the publish flip.
        $ws.ModifySource('pkg')
        $ws.AddCommit('drive-by source edit')
        $beforeFlipSha = $ws.GitSha('HEAD')

        # Rewrite Cargo.toml to use the dotted-key inheritance form.
        $cargoPath = Join-Path $ws.Path 'crates\pkg\Cargo.toml'
        $content = [System.IO.File]::ReadAllText($cargoPath)
        $content = $content -replace 'publish = false', 'publish.workspace = true'
        [System.IO.File]::WriteAllText($cargoPath, $content)
        $ws.AddCommit('flip publish to dotted-key workspace inheritance')
        $flipSha = $ws.GitSha('HEAD')

        $sha = Get-PackageLastReleaseBaseline -RepoRoot $ws.Path -PackageFolder 'pkg'
        $sha | Should -Be $flipSha
        $sha | Should -Not -Be $beforeFlipSha
    }

    It 'recognises a commit that flips publish.workspace = true → publish = false as a baseline reset' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'pkg'; Version = '0.1.0'; PublishSyntax = 'dotted-workspace' }
            )
            WorkspacePackage = [ordered]@{ publish = 'true' }
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'pubflip-dotted-to-literal')

        $ws.ModifySource('pkg')
        $ws.AddCommit('drive-by source edit')

        $cargoPath = Join-Path $ws.Path 'crates\pkg\Cargo.toml'
        $content = [System.IO.File]::ReadAllText($cargoPath)
        $content = $content -replace 'publish\.workspace = true', 'publish = false'
        [System.IO.File]::WriteAllText($cargoPath, $content)
        $ws.AddCommit('flip publish to literal false')
        $flipSha = $ws.GitSha('HEAD')

        $sha = Get-PackageLastReleaseBaseline -RepoRoot $ws.Path -PackageFolder 'pkg'
        $sha | Should -Be $flipSha
    }

    It 'recognises a commit that flips inline-table publish = { workspace = true } to a different value as a baseline reset' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'pkg'; Version = '0.1.0'; PublishSyntax = 'inline-workspace' }
            )
            WorkspacePackage = [ordered]@{ publish = 'true' }
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'pubflip-inline-to-literal')

        $ws.ModifySource('pkg')
        $ws.AddCommit('drive-by source edit')

        $cargoPath = Join-Path $ws.Path 'crates\pkg\Cargo.toml'
        $content = [System.IO.File]::ReadAllText($cargoPath)
        $content = $content -replace 'publish = \{ workspace = true \}', 'publish = false'
        [System.IO.File]::WriteAllText($cargoPath, $content)
        $ws.AddCommit('flip publish to literal false')
        $flipSha = $ws.GitSha('HEAD')

        $sha = Get-PackageLastReleaseBaseline -RepoRoot $ws.Path -PackageFolder 'pkg'
        $sha | Should -Be $flipSha
    }

    It 'treats a commit that adds version.workspace = true as a baseline reset' {
        Reset-ReleaseScriptCaches
        # Start with a literal version line, then rewrite to the dotted-key form.
        $spec = @{
            Packages = @(
                @{ Name = 'pkg'; Version = '0.1.0' }
            )
            WorkspacePackage = [ordered]@{ version = '"0.1.0"' }
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'verflip-literal-to-dotted')

        $ws.ModifySource('pkg')
        $ws.AddCommit('drive-by source edit')

        $cargoPath = Join-Path $ws.Path 'crates\pkg\Cargo.toml'
        $content = [System.IO.File]::ReadAllText($cargoPath)
        $content = $content -replace 'version = "0\.1\.0"', 'version.workspace = true'
        [System.IO.File]::WriteAllText($cargoPath, $content)
        $ws.AddCommit('flip version to dotted-key workspace inheritance')
        $flipSha = $ws.GitSha('HEAD')

        $sha = Get-PackageLastReleaseBaseline -RepoRoot $ws.Path -PackageFolder 'pkg'
        $sha | Should -Be $flipSha
    }
}

Describe 'Get-WorkspacePackages: publish-syntax variants via cargo metadata' {

    # cargo metadata normalises every publish-key syntax (literal,
    # inline-table-workspace, dotted-key-workspace, array) into a single
    # `publish` field on the package object. Get-WorkspacePackages relies on
    # that normalisation, so we exercise the variants end-to-end.

    It 'reports Published=$true for publish.workspace = true with workspace publish = true' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'pkg'; Version = '0.1.0'; PublishSyntax = 'dotted-workspace' }
            )
            WorkspacePackage = [ordered]@{ publish = 'true' }
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'pub-dotted-true')

        $packages = Get-WorkspacePackages -repoRoot $ws.Path
        ($packages | Where-Object { $_.Name -eq 'pkg' }).Published | Should -BeTrue
    }

    It 'reports Published=$false for publish.workspace = true with workspace publish = false' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'pkg'; Version = '0.1.0'; PublishSyntax = 'dotted-workspace' }
            )
            WorkspacePackage = [ordered]@{ publish = 'false' }
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'pub-dotted-false')

        $packages = Get-WorkspacePackages -repoRoot $ws.Path
        ($packages | Where-Object { $_.Name -eq 'pkg' }).Published | Should -BeFalse
    }

    It 'reports Published=$true for publish = { workspace = true } with workspace publish = true' {
        Reset-ReleaseScriptCaches
        $spec = @{
            Packages = @(
                @{ Name = 'pkg'; Version = '0.1.0'; PublishSyntax = 'inline-workspace' }
            )
            WorkspacePackage = [ordered]@{ publish = 'true' }
        }
        $ws = New-SyntheticWorkspace -Spec $spec -Path (Join-Path $TestDrive 'pub-inline-true')

        $packages = Get-WorkspacePackages -repoRoot $ws.Path
        ($packages | Where-Object { $_.Name -eq 'pkg' }).Published | Should -BeTrue
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
        # Mixed6 wires a normal dep on dependency_b and a dev dep on dependency_a;
        # Get-WorkspacePackages flattens to normal/build only.
        $target.Deps | Should -Contain 'dependency_b'
        $target.Deps | Should -Not -Contain 'dependency_a'
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
        # Use Mixed6 — utility is publish=false and depends on dependent_y.
        Reset-ReleaseScriptCaches
        $mixed = New-SyntheticWorkspace -Preset Mixed6 -Path (Join-Path $TestDrive 'transitive-mixed')
        $deps = Get-AllTransitiveDependents -packageName 'dependent_y' -repoRoot $mixed.Path
        # utility depends on dependent_y but is publish=false; should not appear.
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

    It 'rejects empty BaseRef (mandatory parameter)' {
        $ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive ('pend-norefs-' + [guid]::NewGuid().Guid.Substring(0,8)))
        $ws.SetVersion('a', '0.1.1')

        { Get-PendingReleases -RepoRoot $ws.Path -BaseRef '' } | Should -Throw
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
