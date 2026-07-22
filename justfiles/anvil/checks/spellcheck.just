# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# Modified tier.
#
# cargo-spellcheck reads a Hunspell-compatible dictionary file at the
# path configured in spellcheck.toml (typically `extra_dictionaries =
# ["target/spelling.dic"]`). The convention used by the surveyed
# Microsoft Rust repos is to keep the *source* word list in a
# human-edited `.spelling` file at the repo root and preprocess it
# into the .dic format at check time (Hunspell .dic requires:
# alphabetical sort, blank/numeric lines removed, line-count header).
# We always (re)generate `target/spelling.dic` so that a spellcheck.toml
# pointing at it never fails on a missing file -- if `.spelling` is
# absent we emit an empty dictionary (count `0`, no words) rather than
# leaving the path dangling.
[script("pwsh")]
anvil-spellcheck: anvil-spellcheck-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') {
        Write-Host 'anvil-spellcheck: no modified packages; skipping'
        exit 0
    }
    $output_file = 'target/spelling.dic'
    $filtered_lines = @()
    if (Test-Path '.spelling') {
        $lines = Get-Content '.spelling' | Sort-Object
        $filtered_lines = @($lines | Where-Object { $_ -notmatch '^\d+$' -and $_ -ne '' })
    }
    $line_count = $filtered_lines.Count
    [System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($output_file)) | Out-Null
    @($line_count) + $filtered_lines | Set-Content $output_file
    # Pass --cfg explicitly when a spellcheck.toml exists at repo root,
    # otherwise cargo-spellcheck falls back to its built-in defaults and
    # ignores user-curated dictionaries (`extra_dictionaries`, custom
    # hunspell langs, etc.).
    if (Test-Path 'spellcheck.toml') {
        cargo spellcheck --cfg spellcheck.toml check --code 1
    } else {
        cargo spellcheck check --code 1
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# cargo-spellcheck has a build-time libclang dependency; the system
# deps check runs first so adopters get a clear hint instead of a
# cryptic clang-sys build error mid-install.
[group("anvil-setup")]
anvil-spellcheck-setup installer="install": anvil-system-deps-check (anvil-tool-cargo-spellcheck-install installer)

[group("anvil-setup")]
anvil-spellcheck-validate-prereqs: anvil-tool-cargo-spellcheck-validate-prereqs
