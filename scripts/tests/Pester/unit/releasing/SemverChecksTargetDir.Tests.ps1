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
        $env:OXIDIZER_SEMVER_TARGET_DIR = 'D:\short'
        Get-SemverChecksTargetDir | Should -Be 'D:\short'
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
        $script:legacy = Join-Path $script:repo 'target\semver-checks'
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
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir 'C:\ox-semver' 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeFalse
    }

    It 'leaves the directory alone when the target dir IS the repository target dir' {
        # That is the opt-back-out configuration, where this is the live scratch.
        $repoTarget = Join-Path $script:repo 'target'
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $repoTarget 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeTrue
    }

    It 'leaves the directory alone when the target dir is nested inside the repository target dir' {
        $nested = Join-Path $script:repo 'target\custom'
        Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir $nested 6>$null

        Test-Path -LiteralPath $script:legacy | Should -BeTrue
    }

    It 'is a no-op when there is no scratch directory to remove' {
        Remove-Item -LiteralPath $script:legacy -Recurse -Force
        { Clear-LegacySemverChecksScratch -RepoRoot $script:repo -TargetDir 'C:\ox-semver' 6>$null } | Should -Not -Throw
    }
}
