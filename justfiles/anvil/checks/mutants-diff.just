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
    # Three-dot form: only changes reachable from HEAD since it diverged from
    # `$base`. Two-dot `$base..HEAD` on GitHub's merge ref also includes every
    # commit that landed on the base branch after the PR opened, which falsely
    # mutates unrelated packages and fails the gate on their pre-existing gaps.
    git diff "$base...HEAD" --output=$diff_path
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    # Platform-conditional exclusions, layered on top of the static
    # `exclude_globs` in `.cargo/mutants.toml` (cargo-mutants merges the two).
    # cargo-mutants selects mutants by parsing source text, so it does not
    # honour `cfg` gating: code that the compiler strips on this platform still
    # yields mutants, and every one of them is unbuildable and therefore
    # reported as missed. Exclude such code explicitly. Keep this in sync with
    # `scripts/mutants.rs`, which applies the same policy to the merge-group run.
    #
    # Each glob must be written as a literal, quoted argument of the invocation.
    # On Unix, PowerShell expands wildcards in native command arguments against
    # the file system unless the argument is a quoted token, and it does so even
    # for arguments supplied from a variable or a splatted array -- which would
    # turn a directory glob into a list of paths cargo-mutants rejects.
    #
    # A bare name such as `linux` matches any path component, so a module
    # directory holding platform-gated code is excluded by naming it. The file
    # globs omit the separator before the platform name so that both `linux.rs`
    # and `socket_linux.rs` match.
    #
    # `fetch_winhttp_impl`'s integration tests live in the facade package, so
    # when the diff touches either package the facade must be listed as a test
    # package or every mutant is scored against unit tests alone. Match path
    # headers in the unified diff (`+++ b/crates/...`) rather than any mention
    # of the path in hunk text.
    $diff_text = [System.IO.File]::ReadAllText($diff_path)
    $test_packages = @()
    if ($diff_text -match '(?m)^(\+\+\+|---) [ab]/crates[/\\]fetch_winhttp(_impl)?[/\\]') {
        $test_packages = @('--test-package', 'fetch_winhttp', '--test-package', 'fetch_winhttp_impl')
    }
    if ($IsWindows) {
        cargo mutants --in-diff $diff_path --no-shuffle --jobs 0 `
            --exclude "**/*linux.rs" --exclude "linux" `
            @test_packages
    } else {
        # `fetch_winhttp` implements nothing outside Windows, so the whole crate
        # goes rather than individual files, and so does the implementation crate
        # behind it.
        cargo mutants --in-diff $diff_path --no-shuffle --jobs 0 `
            --exclude "**/*windows.rs" --exclude "windows" `
            --exclude "crates/fetch_winhttp/**" `
            --exclude "crates/fetch_winhttp_impl/**" `
            @test_packages
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# --- pr-mutants members ---

# Install prerequisites for the `anvil-mutants-diff` recipe.
[group("anvil-setup")]
anvil-mutants-diff-setup installer="install": (anvil-tool-cargo-mutants-install installer)

# Validate prerequisites for the `anvil-mutants-diff` recipe.
[group("anvil-setup")]
anvil-mutants-diff-validate-prereqs: anvil-tool-cargo-mutants-validate-prereqs
