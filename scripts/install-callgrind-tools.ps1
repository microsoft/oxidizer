# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# Installs the Callgrind benchmark toolchain (gungraun-runner). Linux-only:
# Callgrind requires Valgrind, which is Linux-only. The runner is the helper
# binary that Callgrind bench binaries (built with `harness = false`) hand
# their work off to via an encoded payload.
#
# Keep the version in lockstep with the `gungraun` workspace dependency.
# `gungraun-runner` enforces strict string equality on the version
# (`gungraun-runner::runner::compare_versions`), so any patch-level drift
# between the library and the runner causes `*_cg` benches to fail at runtime
# with VersionMismatch.

param(
    [ValidateSet('install', 'binstall')]
    [string]$Installer = 'install',
    [string[]]$CargoArguments = @(),
    [switch]$ValidateOnly
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false

function Initialize-CallgrindTools {
    param(
        [Parameter(Mandatory)][string]$RequiredVersion,
        [string]$Installer = 'install',
        [string[]]$CargoArguments = @(),
        [switch]$ValidateOnly
    )

    $expected = "gungraun-runner $RequiredVersion"
    $installed = $null
    if (Get-Command gungraun-runner -ErrorAction SilentlyContinue) {
        $installed = (& gungraun-runner --version | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) { throw 'Could not read the installed gungraun-runner version.' }
    }

    if ($ValidateOnly) {
        if ($installed -cne $expected) {
            throw "gungraun-runner $RequiredVersion is required (installed: $installed). Run: just anvil-examples-setup"
        }
        if (-not (Get-Command valgrind -ErrorAction SilentlyContinue)) {
            throw 'Valgrind is required for Linux examples. On Ubuntu/Debian, run: sudo apt-get install -y valgrind'
        }
        return
    }
    if ($installed -ceq $expected) {
        Write-Host "$expected is already installed."
        return
    }

    $installArguments = @('--locked', '--force', '--version', "=$RequiredVersion", 'gungraun-runner')
    if ($Installer -eq 'binstall') {
        & cargo @CargoArguments binstall --no-confirm @installArguments
        if ($LASTEXITCODE -eq 0) { return }
        Write-Warning 'gungraun-runner binary installation failed; falling back to cargo install.'
    }
    & cargo @CargoArguments install @installArguments
    if ($LASTEXITCODE -ne 0) { throw "Could not install gungraun-runner $RequiredVersion." }
}

if (-not $IsLinux) {
    Write-Host "Callgrind toolchain is Linux-only; skipping."
    return
}

$manifest = Get-Content (Join-Path $PSScriptRoot '..' 'Cargo.toml') -Raw
$match = [regex]::Match($manifest, '(?m)^gungraun\s*=\s*\{[^}]*version\s*=\s*"=([^"]+)"')
if (-not $match.Success) {
    throw 'Could not determine the exact gungraun version from Cargo.toml.'
}

Initialize-CallgrindTools -RequiredVersion $match.Groups[1].Value -Installer $Installer -CargoArguments $CargoArguments -ValidateOnly:$ValidateOnly
