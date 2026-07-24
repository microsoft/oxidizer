# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Affected tier. cargo-mutants does its own diff-scoping via --in-diff;
# the affected-tier guard is the wiring-layer's coarse filter (if no
# affected packages exist, the entire mutants run is pointless).
#
# Skip on aarch64-pc-windows-msvc: cargo-mutants doesn't build there
# (upstream winapi incompatibility), so `_anvil-require cargo-mutants`
# would fail. The merged pr-slow group runs on all four OS legs; mutants
# is the only sub-recipe that can't follow, so it bails out early on the
# affected leg. Coverage on the other three legs is unchanged.

# Run mutation testing for changes relative to the base branch.
[script("pwsh")]
anvil-mutants-diff: anvil-mutants-diff-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($IsWindows -and ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64')) {
        Write-Host 'anvil-mutants-diff: anvil-mutants-diff-validate-prereqs aarch64-pc-windows-msvc -- cargo-mutants does not build here (winapi); skipping'
        exit 0
    }
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-mutants-diff: anvil-mutants-diff-validate-prereqs no affected packages; skipping'
        exit 0
    }
    # Resolve the diff base via the shared resolver (single source of truth
    # across the impact steps and this recipe; see _anvil-base-ref).
    $base = (& "{{just_executable()}}" _anvil-base-ref)
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    # cargo-mutants --in-diff takes a FILE path containing a unified
    # diff, not a git revision range. Write the diff to a temp file
    # first. RUNNER_TEMP (GH) and AGENT_TEMPDIRECTORY (ADO) point at
    # the job's scratch dir; fall back to the system temp dir locally.
    $tmp_dir = $env:RUNNER_TEMP
    if (-not $tmp_dir) { $tmp_dir = $env:AGENT_TEMPDIRECTORY }
    if (-not $tmp_dir) { $tmp_dir = [System.IO.Path]::GetTempPath() }
    $diff_path = Join-Path $tmp_dir 'anvil-mutants-diff.diff'
    git diff "$base..HEAD" --output=$diff_path
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo mutants --in-diff $diff_path --no-shuffle --jobs 0
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# --- pr-mutants members ---

# Install prerequisites for the `anvil-mutants-diff` recipe.
[group("anvil-setup")]
anvil-mutants-diff-setup installer="install": (anvil-tool-cargo-mutants-install installer)

# Validate prerequisites for the `anvil-mutants-diff` recipe.
[group("anvil-setup")]
anvil-mutants-diff-validate-prereqs: anvil-tool-cargo-mutants-validate-prereqs
