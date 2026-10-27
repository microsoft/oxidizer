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

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

if (-not $IsLinux) {
    Write-Host "Callgrind toolchain is Linux-only; skipping."
    return
}

$manifest = Get-Content (Join-Path $PSScriptRoot '..' 'Cargo.toml') -Raw
$match = [regex]::Match($manifest, '(?m)^gungraun\s*=\s*\{[^}]*version\s*=\s*"=([^"]+)"')
if (-not $match.Success) {
    throw 'Could not determine the exact gungraun version from Cargo.toml.'
}

cargo install --locked gungraun-runner --version $match.Groups[1].Value
