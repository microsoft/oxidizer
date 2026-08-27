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

Describe 'relocated target directory' {
    # The linker override cannot help the C compilers that -sys crates drive
    # through the cc crate: MSVC cl.exe resolves its -Fo argument against
    # MAX_PATH, and there is no long-path-aware drop-in guaranteed to be
    # installed. The unit tests pin the environment-variable lifecycle; this
    # exercises the claim underneath it, that the ceiling is real and that
    # shortening the root is what clears it.
    It 'compiles at a depth that defeats cl.exe once the root is shortened' -Skip:(-not $IsWindows) {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (-not (Test-Path -LiteralPath $vswhere)) {
            Set-ItResult -Skipped -Because 'vswhere is absent, so no MSVC toolchain can be located'
            return
        }

        $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath |
            Select-Object -First 1
        $vcvars = if ($vsPath) { Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat' } else { $null }
        if (-not $vcvars -or -not (Test-Path -LiteralPath $vcvars)) {
            Set-ItResult -Skipped -Because 'no MSVC C toolchain is installed'
            return
        }

        $root = Join-Path ([System.IO.Path]::GetTempPath()) ('oxi-cl-' + [guid]::NewGuid().ToString('n').Substring(0, 8))
        $null = New-Item -ItemType Directory -Path $root -Force

        try {
            # No #include, so the compile needs nothing from INCLUDE and the only
            # variable under test is the length of the output path.
            $source = Join-Path $root 'probe.c'
            Set-Content -LiteralPath $source -Value 'int probe(void){return 42;}' -Encoding ascii

            $deep = $root
            while ($deep.Length -lt 240) {
                $deep = Join-Path $deep 'nested-directory-segment'
            }
            # CreateDirectory rather than New-Item: the \\?\ prefix is what lets
            # this exceed MAX_PATH, and `?` is a wildcard to PowerShell's -Path,
            # for which New-Item offers no -LiteralPath counterpart.
            $null = [System.IO.Directory]::CreateDirectory("\\?\$deep")

            # Named after the object that failed in the field, so the reproduction
            # keeps the same shape as the report.
            $leaf = 'a9466447ad5a187b-jitterentropy-health.o'
            $deepObj = Join-Path $deep $leaf
            $deepObj.Length | Should -BeGreaterThan 260 -Because 'the premise is a path MSVC cannot open'

            $compile = {
                param($outPath)
                # A failing compile is the premise of the first half of this
                # test, not an error: the suite runner turns non-zero native
                # exits into terminating errors, so opt out for these calls.
                $PSNativeCommandUseErrorActionPreference = $false
                $line = "call `"$vcvars`" >nul && cl.exe -nologo -c `"$source`" `"-Fo$outPath`""
                $text = & $env:ComSpec /c $line 2>&1 | Out-String
                [pscustomobject]@{ Output = $text; ExitCode = $LASTEXITCODE }
            }

            $deepResult = & $compile $deepObj
            if ($deepResult.ExitCode -eq 0) {
                Set-ItResult -Skipped -Because 'this toolchain opens long output paths, so the relocation has nothing to prove here'
                return
            }

            # C1083 specifically: a laxer pattern would let an unrelated failure
            # satisfy the premise and make the assertion below vacuous.
            $deepResult.Output | Should -Match 'C1083'

            # Now the same compile beneath the directory the release scripts pick.
            $shortRoot = Get-SemverChecksTargetDirPath -RepoRoot $root
            $shortDir = Join-Path $shortRoot 'probe'
            $null = New-Item -ItemType Directory -Path $shortDir -Force
            try {
                $shortResult = & $compile (Join-Path $shortDir $leaf)

                $shortResult.ExitCode | Should -Be 0 -Because "cl.exe should compile under the short root, but said: $($shortResult.Output)"
                $shortResult.Output | Should -Not -Match 'C1083'
            } finally {
                Remove-Item -LiteralPath $shortRoot -Recurse -Force -ErrorAction SilentlyContinue
            }
        } finally {
            Remove-Item -LiteralPath "\\?\$root" -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
