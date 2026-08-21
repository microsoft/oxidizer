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

Describe 'Invoke-SemverChecksCli' {
    BeforeEach {
        $script:envName = 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER'
        $script:savedLinker = if (Test-Path "Env:\$script:envName") { (Get-Item "Env:\$script:envName").Value } else { $null }
        Remove-Item "Env:\$script:envName" -ErrorAction SilentlyContinue
    }

    AfterEach {
        if ($null -ne $script:savedLinker) {
            Set-Item "Env:\$script:envName" -Value $script:savedLinker
        } else {
            Remove-Item "Env:\$script:envName" -ErrorAction SilentlyContinue
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
}
