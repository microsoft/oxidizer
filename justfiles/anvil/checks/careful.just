# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

[script("pwsh")]
anvil-careful: anvil-careful-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Guard against cargo-careful's stale-sysroot hazard. cargo-careful
    # builds a custom std ("careful sysroot": debug-assertions + extra UB
    # checks) into a STABLE cache path -- ProjectDirs(de.ralfj.cargo-careful),
    # NOT keyed by rustc version -- and runs the workspace build with
    # `--sysroot <that stable path>`. cargo's fingerprint hashes the
    # RUSTFLAGS string (the stable sysroot *path*) but not the sysroot's
    # *contents*. So when the pinned nightly (or the careful tool, which sets
    # the std-build flags) changes, cargo-careful rebuilds that std in place
    # while cargo still considers the workspace artifacts up to date -- they
    # then link against a now-replaced std and fail with metadata /
    # "found crate compiled by an incompatible version of rustc" errors.
    #
    # cargo can't see this, so we track the careful build identity ourselves
    # (the nightly rustc -- commit hash et al. -- plus the careful tool
    # version) and `cargo clean` when it changes, forcing the workspace to be
    # rebuilt against the freshly-built std. The marker lives under target/,
    # so it is wiped by the clean and travels with any target/ cache.
    $rustcVV = (& rustc '+{{ rust_nightly }}' -vV | Out-String)
    if ($LASTEXITCODE -ne 0) { Write-Error 'anvil-careful: could not query nightly rustc version'; exit 1 }
    $idSource = $rustcVV.Trim() + "`ncargo-careful {{ cargo_careful_version }}"
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $idHash = ([System.BitConverter]::ToString($sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($idSource)))).Replace('-', '').ToLower()
    # Human-readable identity for log output: the first `rustc -vV` line
    # (e.g. "rustc 1.98.0-nightly (<hash> <date>)") plus the careful version.
    $idLabel = "$(($rustcVV -split "`n")[0].Trim()) + cargo-careful {{ cargo_careful_version }}"
    $marker = Join-Path 'target' 'anvil/careful-sysroot.id'
    $prev = if (Test-Path -LiteralPath $marker) { (Get-Content -LiteralPath $marker -Raw).Trim() } else { $null }
    if ($null -ne $prev -and $prev -ne $idHash) {
        Write-Host 'anvil-careful: careful sysroot identity changed since the last run -- running `cargo clean` so the'
        Write-Host '  workspace is rebuilt against the freshly-built careful std (cargo cannot detect the in-place'
        Write-Host '  sysroot rebuild on its own, so stale artifacts would otherwise fail with an'
        Write-Host '  "incompatible version of rustc" / metadata mismatch).'
        Write-Host "  reason : the pinned nightly toolchain and/or the cargo-careful version changed"
        Write-Host "  now    : $idLabel"
        Write-Host "  identity: $($prev.Substring(0, [Math]::Min(12, $prev.Length)))... -> $($idHash.Substring(0, 12))..."
        cargo clean
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
    # Persist the current identity (also seeds the marker on first run, when
    # there is nothing to clean). Written after the clean so it survives it.
    New-Item -ItemType Directory -Force -Path (Split-Path $marker) | Out-Null
    Set-Content -LiteralPath $marker -Value $idHash
    & cargo '+{{ rust_nightly }}' careful test @pkg --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-careful-setup installer="install": anvil-component-nightly-rust-src-install (anvil-tool-cargo-careful-install installer)

[group("anvil-setup")]
anvil-careful-validate-prereqs: anvil-component-nightly-rust-src-validate-prereqs anvil-tool-cargo-careful-validate-prereqs
