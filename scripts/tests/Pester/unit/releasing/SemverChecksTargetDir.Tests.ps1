# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
}

Describe 'Get-SemverChecksTargetDir' {
    BeforeEach {
        $script:savedOverride = if (Test-Path Env:\OXIDIZER_SEMVER_TARGET_DIR) { $env:OXIDIZER_SEMVER_TARGET_DIR } else { $null }
        Remove-Item Env:\OXIDIZER_SEMVER_TARGET_DIR -ErrorAction SilentlyContinue
    }

    AfterEach {
        if ($null -ne $script:savedOverride) {
            $env:OXIDIZER_SEMVER_TARGET_DIR = $script:savedOverride
        } else {
            Remove-Item Env:\OXIDIZER_SEMVER_TARGET_DIR -ErrorAction SilentlyContinue
        }
    }

    It 'honors an explicit OXIDIZER_SEMVER_TARGET_DIR override on every platform' {
        # Must be an absolute path *for the current platform*: 'D:\short' is not
        # fully qualified on Linux or macOS, so hardcoding it would both assert
        # the wrong contract and fail there.
        $absolute = Join-Path ([System.IO.Path]::GetTempPath()) 'ox-semver-override'
        $env:OXIDIZER_SEMVER_TARGET_DIR = $absolute
        Get-SemverChecksTargetDir | Should -Be $absolute
    }

    It 'rejects a relative override rather than resolving it against the working directory' {
        # A relative path resolves differently in this process and in the cargo
        # child process, so accepting it would recreate the class of failure
        # this scratch directory exists to avoid.
        $env:OXIDIZER_SEMVER_TARGET_DIR = 'ox-semver'
        { Get-SemverChecksTargetDir } | Should -Throw '*must be an absolute path*'
    }

    It 'rejects a drive-relative override on Windows' -Skip:(-not $IsWindows) {
        # 'C:ox-semver' looks absolute but resolves against the per-drive
        # current directory.
        $env:OXIDIZER_SEMVER_TARGET_DIR = 'C:ox-semver'
        { Get-SemverChecksTargetDir } | Should -Throw '*must be an absolute path*'
    }

    It 'ignores a whitespace-only override' {
        $env:OXIDIZER_SEMVER_TARGET_DIR = '   '
        $result = Get-SemverChecksTargetDir

        # Falls through to the platform default rather than returning blanks,
        # which cargo would treat as a set-but-empty CARGO_TARGET_DIR.
        $result | Should -Not -Be '   '
    }

    It 'returns a short scratch root on Windows and nothing elsewhere' {
        $result = Get-SemverChecksTargetDir

        if ($IsWindows) {
            # The whole point is headroom against MAX_PATH: the scratch root must
            # stay far shorter than the ~210-character tail cargo-semver-checks
            # generates beneath it (AB#7696711).
            $result | Should -Not -BeNullOrEmpty
            $result.Length | Should -BeLessThan 24
        } else {
            # No MAX_PATH constraint: keep Cargo's default so the existing
            # repository-local target directory and its cache are reused.
            $result | Should -BeNullOrEmpty
        }
    }
}

Describe 'Clear-LegacySemverChecksScratch' {
    BeforeEach {
        $script:repo = Join-Path ([System.IO.Path]::GetTempPath()) ("clsc-" + [guid]::NewGuid().ToString('N'))
        # Build paths segment by segment: embedding a separator would bake in
        # Windows semantics and mask separator bugs on Linux/macOS.
        $script:repoTarget = Join-Path $script:repo 'target'
        $script:legacy = Join-Path $script:repoTarget 'semver-checks'
        [void][System.IO.Directory]::CreateDirectory($script:legacy)
        Set-Content -LiteralPath (Join-Path $script:legacy 'marker.txt') -Value 'x'
    }

    AfterEach {
        if (Test-Path -LiteralPath $script:repo) {
            Remove-Item -LiteralPath $script:repo -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    It 'removes the in-repo scratch directory when the target dir is redirected elsewhere' {
        # Otherwise its baseline clones become visible workspace source and
        # cargo fails with `package <name> is ambiguous`.
        $elsewhere = Join-Path ([System.IO.Path]::GetTempPath()) 'ox-semver-elsewhere'
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $elsewhere 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeFalse
    }

    It 'leaves the directory alone when the target dir IS the repository target dir' {
        # That is the opt-back-out configuration, where this is the live scratch.
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $script:repoTarget 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeTrue
    }

    It 'leaves the directory alone when the target dir is nested inside the repository target dir' {
        $nested = Join-Path $script:repoTarget 'custom'
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $nested 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeTrue
    }

    It 'leaves the directory alone when dot segments resolve inside the repository target dir' {
        $nestedViaDot = Join-Path (Join-Path $script:repoTarget '.') 'custom'
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $nestedViaDot 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeTrue
    }

    It 'leaves the directory alone when the target dir is the scratch directory itself' {
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $script:legacy 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeTrue
    }

    It 'removes the scratch dir when parent segments resolve to a sibling' {
        $siblingViaParent = Join-Path (Join-Path $script:repoTarget '..') 'semver-out'
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $siblingViaParent 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeFalse
    }

    It 'still removes the scratch dir for a sibling path that merely shares a prefix' {
        # `<repo>/target-scratch` is NOT inside `<repo>/target`, so the guard
        # must not treat a bare string prefix as containment.
        $sibling = Join-Path $script:repo 'target-scratch'
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $sibling 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeFalse
    }

    It 'is a no-op when there is no scratch directory to remove' {
        Remove-Item -LiteralPath $script:legacy -Recurse -Force
        $elsewhere = Join-Path ([System.IO.Path]::GetTempPath()) 'ox-semver-elsewhere'
        { Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $elsewhere 6>$null } | Should -Not -Throw
    }
}
