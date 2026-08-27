# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
}

Describe 'Get-SemverChecksLinkerEnvName' {
    It 'names the Cargo linker variable for the host triple on Windows' -Skip:(-not $IsWindows) {
        $name = Get-SemverChecksLinkerEnvName

        $name | Should -Match '^CARGO_TARGET_[A-Z0-9_]+_LINKER$'
        # The separators of the host triple must survive as underscores.
        $name | Should -BeLike '*_PC_WINDOWS_*'
    }

    It 'reads the triple from the host line, not from any other rustc field' -Skip:(-not $IsWindows) {
        Mock rustc {
            @'
rustc 1.90.0 (aaaaaaaaa 2026-01-01)
binary: rustc
commit-hash: aaaaaaaaa
release: 1.90.0
host: aarch64-pc-windows-msvc
LLVM version: 20.1.0
'@
        }

        Get-SemverChecksLinkerEnvName | Should -Be 'CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER'
    }

    It 'returns nothing when rustc reports no host line' -Skip:(-not $IsWindows) {
        Mock rustc { 'rustc 1.90.0' }

        Get-SemverChecksLinkerEnvName -WarningAction SilentlyContinue | Should -BeNullOrEmpty
    }

    It 'warns when it cannot determine the host triple' -Skip:(-not $IsWindows) {
        Mock rustc { 'rustc 1.90.0' }

        Get-SemverChecksLinkerEnvName -WarningVariable warnings -WarningAction SilentlyContinue | Out-Null

        $warnings | Should -Not -BeNullOrEmpty
        "$warnings" | Should -BeLike '*LNK1104*'
    }

    It 'leaves the toolchain default in place on a windows-gnu host' -Skip:(-not $IsWindows) {
        # rust-lld would displace the gcc driver there, which this fix does not need.
        Mock rustc { 'host: x86_64-pc-windows-gnu' }

        Get-SemverChecksLinkerEnvName | Should -BeNullOrEmpty
    }

    It 'ignores a stale exit code left by an earlier native command' -Skip:(-not $IsWindows) {
        # $LASTEXITCODE only tracks native executables, so it survives calls that
        # never touch it. The override must key off the parsed output instead.
        Mock rustc { 'host: x86_64-pc-windows-msvc' }
        $savedExitCode = $global:LASTEXITCODE
        try {
            $global:LASTEXITCODE = 7

            Get-SemverChecksLinkerEnvName | Should -Be 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER'
        } finally {
            $global:LASTEXITCODE = $savedExitCode
        }
    }

    It 'returns nothing when rustc cannot be run at all' -Skip:(-not $IsWindows) {
        Mock rustc { throw 'rustc is not installed' }

        Get-SemverChecksLinkerEnvName | Should -BeNullOrEmpty
    }

    It 'leaves the toolchain default in place on non-Windows platforms' -Skip:$IsWindows {
        Get-SemverChecksLinkerEnvName | Should -BeNullOrEmpty
    }
}

Describe 'Get-SemverChecksTargetDirPath' {
    It 'roots the build at the volume of the repository on Windows' -Skip:(-not $IsWindows) {
        $path = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer.worktrees\some-long-branch-name'

        $path | Should -BeLike 'C:\oxi-sc\*'
    }

    It 'stays short enough to leave the MAX_PATH budget to the build itself' -Skip:(-not $IsWindows) {
        # The observed worst case nests ~209 characters of cargo-semver-checks
        # and aws-lc-sys build output beneath the target directory, so the root
        # has to stay well clear of 260.
        $path = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer'

        $path.Length | Should -BeLessOrEqual 20
    }

    It 'keeps separate clones in separate directories' -Skip:(-not $IsWindows) {
        $first = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer'
        $second = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer-two'

        $first | Should -Not -Be $second
    }

    It 'returns the same directory for the same clone across runs' -Skip:(-not $IsWindows) {
        # Determinism is what lets a rerun reuse the baseline rustdoc it just built.
        $first = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer'
        $second = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer'

        $first | Should -Be $second
    }

    It 'treats equivalent spellings of one root as the same clone' -Skip:(-not $IsWindows) {
        # Windows does not distinguish these, so the digest must not either.
        $plain = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer'
        $trailing = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\oxidizer\'
        $cased = Get-SemverChecksTargetDirPath -RepoRoot 'C:\SOURCE\Oxidizer'
        $relative = Get-SemverChecksTargetDirPath -RepoRoot 'C:\Source\other\..\oxidizer'

        $trailing | Should -Be $plain
        $cased    | Should -Be $plain
        $relative | Should -Be $plain
    }

    It 'follows the repository to another volume' -Skip:(-not $IsWindows) {
        # Staying on the developer's chosen filesystem avoids a cross-volume copy.
        $path = Get-SemverChecksTargetDirPath -RepoRoot 'D:\oxidizer'

        $path | Should -BeLike 'D:\oxi-sc\*'
    }

    It 'falls back to the system drive for a UNC root' -Skip:(-not $IsWindows) {
        # A UNC root has no drive letter to shorten to and would keep the very
        # length the relocation exists to shed.
        $path = Get-SemverChecksTargetDirPath -RepoRoot '\\server\share\oxidizer'

        $path | Should -BeLike "$env:SystemDrive\oxi-sc\*"
    }

    It 'leaves the default target directory in place off Windows' -Skip:$IsWindows {
        Get-SemverChecksTargetDirPath -RepoRoot '/home/user/oxidizer' | Should -BeNullOrEmpty
    }
}

Describe 'Invoke-SemverChecksCli' {
    BeforeEach {
        $script:envName = 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER'
        $script:savedLinker = if (Test-Path "Env:\$script:envName") { (Get-Item "Env:\$script:envName").Value } else { $null }
        Remove-Item "Env:\$script:envName" -ErrorAction SilentlyContinue

        $script:savedTargetDir = if (Test-Path 'Env:\CARGO_TARGET_DIR') { $env:CARGO_TARGET_DIR } else { $null }
        Remove-Item 'Env:\CARGO_TARGET_DIR' -ErrorAction SilentlyContinue

        # Default the relocation off. These cases exercise the linker override,
        # and the real path resolves to a volume root that unit tests must not
        # create directories in; the relocation cases below opt back in.
        Mock Get-SemverChecksTargetDirPath { $null }
    }

    AfterEach {
        if ($null -ne $script:savedLinker) {
            Set-Item "Env:\$script:envName" -Value $script:savedLinker
        } else {
            Remove-Item "Env:\$script:envName" -ErrorAction SilentlyContinue
        }

        if ($null -ne $script:savedTargetDir) {
            $env:CARGO_TARGET_DIR = $script:savedTargetDir
        } else {
            Remove-Item 'Env:\CARGO_TARGET_DIR' -ErrorAction SilentlyContinue
        }
    }

    It 'points the linker variable at rust-lld while cargo runs' {
        Mock Get-SemverChecksLinkerEnvName { $script:envName }
        Mock cargo {
            $script:observed = (Get-Item "Env:\$script:envName").Value
            'no semver update required'
        }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        $script:observed | Should -Be 'rust-lld.exe'
    }

    It 'removes the linker variable afterwards when it was not set before' {
        Mock Get-SemverChecksLinkerEnvName { $script:envName }
        Mock cargo { 'no semver update required' }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        Test-Path "Env:\$script:envName" | Should -BeFalse
    }

    It 'keeps an explicitly configured linker instead of overriding it' {
        Set-Item "Env:\$script:envName" -Value 'original-linker'
        Mock Get-SemverChecksLinkerEnvName { $script:envName }
        Mock cargo {
            $script:observed = (Get-Item "Env:\$script:envName").Value
            'no semver update required'
        }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        $script:observed | Should -Be 'original-linker'
        (Get-Item "Env:\$script:envName").Value | Should -Be 'original-linker'
    }

    It 'removes the linker variable when the cargo invocation throws' {
        Mock Get-SemverChecksLinkerEnvName { $script:envName }
        Mock cargo { throw 'cargo exploded' }

        { Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive } |
            Should -Throw '*cargo exploded*'

        Test-Path "Env:\$script:envName" | Should -BeFalse
    }

    It 'probes the toolchain from inside the repository root' {
        # rustup resolves toolchain overrides against the working directory, so
        # probing elsewhere can name a variable for the wrong host triple.
        Mock Get-SemverChecksLinkerEnvName {
            $script:probeLocation = (Get-Location).Path
            $null
        }
        Mock cargo { 'no semver update required' }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        $script:probeLocation | Should -Be ([System.IO.Path]::GetFullPath($TestDrive).TrimEnd('\', '/'))
    }

    It 'leaves the environment untouched when there is no override' {
        Mock Get-SemverChecksLinkerEnvName { $null }
        Mock cargo { 'no semver update required' }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        Test-Path "Env:\$script:envName" | Should -BeFalse
    }

    It 'builds under the relocated target directory while cargo runs' {
        $target = Join-Path $TestDrive 'sc'
        Mock Get-SemverChecksLinkerEnvName { $null }
        Mock Get-SemverChecksTargetDirPath { $target }
        Mock cargo {
            $script:observedTarget = $env:CARGO_TARGET_DIR
            'no semver update required'
        }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        $script:observedTarget | Should -Be $target
    }

    It 'creates the relocated target directory' {
        # cargo would create it too, but only after the probe; creating it here
        # is what turns an unwritable location into a warning rather than a
        # mid-build failure.
        $target = Join-Path $TestDrive 'made-on-demand'
        Mock Get-SemverChecksLinkerEnvName { $null }
        Mock Get-SemverChecksTargetDirPath { $target }
        Mock cargo { 'no semver update required' }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        Test-Path -LiteralPath $target | Should -BeTrue
    }

    It 'removes CARGO_TARGET_DIR afterwards when it was not set before' {
        $target = Join-Path $TestDrive 'sc'
        Mock Get-SemverChecksLinkerEnvName { $null }
        Mock Get-SemverChecksTargetDirPath { $target }
        Mock cargo { 'no semver update required' }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        Test-Path 'Env:\CARGO_TARGET_DIR' | Should -BeFalse
    }

    It 'removes CARGO_TARGET_DIR when the cargo invocation throws' {
        $target = Join-Path $TestDrive 'sc'
        Mock Get-SemverChecksLinkerEnvName { $null }
        Mock Get-SemverChecksTargetDirPath { $target }
        Mock cargo { throw 'cargo exploded' }

        { Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive } |
            Should -Throw '*cargo exploded*'

        Test-Path 'Env:\CARGO_TARGET_DIR' | Should -BeFalse
    }

    It 'keeps an explicitly configured CARGO_TARGET_DIR instead of relocating' {
        $env:CARGO_TARGET_DIR = 'original-target'
        $target = Join-Path $TestDrive 'sc'
        Mock Get-SemverChecksLinkerEnvName { $null }
        Mock Get-SemverChecksTargetDirPath { $target }
        Mock cargo {
            $script:observedTarget = $env:CARGO_TARGET_DIR
            'no semver update required'
        }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive

        $script:observedTarget | Should -Be 'original-target'
        $env:CARGO_TARGET_DIR | Should -Be 'original-target'
    }

    It 'warns and builds in place when the target directory cannot be created' {
        # Fail open: an unwritable volume root must not abort a release.
        Mock Get-SemverChecksLinkerEnvName { $null }
        Mock Get-SemverChecksTargetDirPath { Join-Path $TestDrive 'unwritable' }
        Mock New-Item { throw 'access denied' } -ParameterFilter { $ItemType -eq 'Directory' }
        Mock cargo {
            $script:observedTarget = $env:CARGO_TARGET_DIR
            'no semver update required'
        }

        $null = Invoke-SemverChecksCli -PackageName 'sample' -BaselineSha 'abc' -RepoRoot $TestDrive `
            -WarningVariable warnings -WarningAction SilentlyContinue

        $script:observedTarget | Should -BeNullOrEmpty
        "$warnings" | Should -BeLike '*C1083*'
    }
}
