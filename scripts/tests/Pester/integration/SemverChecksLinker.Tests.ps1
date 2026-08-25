# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
}

Describe 'rust-lld linker override' {
    # The unit tests mock cargo, so they pin the environment-variable lifecycle
    # but not the claim the change rests on: that a bare `rust-lld.exe` resolves
    # and links where the default MSVC linker hits MAX_PATH. This exercises a
    # real toolchain instead. cargo-semver-checks is deliberately not involved --
    # it passes its environment to the build it spawns, which is plain process
    # inheritance, and requiring the tool here would make the test need a
    # network install to say anything.
    It 'links a crate under a target path that defeats the default linker' -Skip:(-not $IsWindows) {
        $linkerVar = Get-SemverChecksLinkerEnvName
        if (-not $linkerVar) {
            Set-ItResult -Skipped -Because 'the host toolchain is not *-msvc, so no override applies'
            return
        }

        $root = Join-Path ([System.IO.Path]::GetTempPath()) ('oxi-lld-' + [guid]::NewGuid().ToString('n').Substring(0, 8))
        $crateDir = Join-Path $root 'crate'

        # A target directory long enough that link.exe's output path exceeds
        # MAX_PATH. The crate itself stays shallow; only the artifacts go deep.
        $deepTarget = Join-Path $root 'target'
        while ($deepTarget.Length -lt 230) {
            $deepTarget = Join-Path $deepTarget 'nested-directory-segment'
        }

        $savedTargetDir = $env:CARGO_TARGET_DIR
        $savedLinker = if (Test-Path "Env:\$linkerVar") { (Get-Item "Env:\$linkerVar").Value } else { $null }

        try {
            $null = New-Item -ItemType Directory -Path (Join-Path $crateDir 'src') -Force

            # An empty [workspace] detaches the crate from any enclosing one, and
            # a binary target is what forces a link step at all.
            Set-Content -LiteralPath (Join-Path $crateDir 'Cargo.toml') -Value @'
[package]
name = "lld-probe"
version = "0.1.0"
edition = "2021"

[workspace]
'@
            Set-Content -LiteralPath (Join-Path $crateDir 'src\main.rs') -Value 'fn main() {}'

            $env:CARGO_TARGET_DIR = $deepTarget
            Remove-Item -Path "Env:\$linkerVar" -ErrorAction SilentlyContinue

            $PSNativeCommandUseErrorActionPreference = $false
            $baseline = & cargo build --quiet --manifest-path (Join-Path $crateDir 'Cargo.toml') 2>&1 | Out-String
            $baselineExit = $LASTEXITCODE

            if ($baselineExit -eq 0) {
                Set-ItResult -Skipped -Because 'this environment links long paths without help, so the override has nothing to prove here'
                return
            }

            # LNK1104 specifically: a laxer pattern would let unrelated build
            # failures satisfy the premise and make the assertion below vacuous.
            $baseline | Should -Match 'LNK1104'

            Set-Item -Path "Env:\$linkerVar" -Value 'rust-lld.exe'
            $overridden = & cargo build --quiet --manifest-path (Join-Path $crateDir 'Cargo.toml') 2>&1 | Out-String
            $overriddenExit = $LASTEXITCODE

            $overriddenExit | Should -Be 0 -Because "rust-lld should link under a long path, but cargo said: $overridden"
            $overridden | Should -Not -Match 'LNK1104'
        } finally {
            $env:CARGO_TARGET_DIR = $savedTargetDir
            if ($null -ne $savedLinker) {
                Set-Item -Path "Env:\$linkerVar" -Value $savedLinker
            } else {
                Remove-Item -Path "Env:\$linkerVar" -ErrorAction SilentlyContinue
            }

            # The \\?\ prefix is what makes the deep tree removable.
            Remove-Item -LiteralPath "\\?\$root" -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
