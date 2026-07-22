# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

# cargo-aprz queries the GitHub advisory API. Unauthenticated access is
# capped at 60 requests/hour and fails on a full run; an authenticated
# token raises the cap to 5000/hour. CI injects GITHUB_TOKEN
# (github.token). For local runs, if GITHUB_TOKEN is unset we borrow the
# gh CLI's stored token (non-interactive: `gh auth token` prints the
# active account's token for github.com and never opens a browser/auth
# prompt). If neither is available we warn with instructions and proceed
# unauthenticated.
#
# Unscoped (consults external risk DB).
[script("pwsh")]
anvil-aprz: anvil-aprz-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if (-not $env:GITHUB_TOKEN) {
        $tok = $null
        if (Get-Command gh -ErrorAction SilentlyContinue) {
            try { $tok = (gh auth token --hostname github.com 2>$null) } catch { $tok = $null }
        }
        if ($tok) {
            $env:GITHUB_TOKEN = $tok.Trim()
        } else {
            Write-Warning 'anvil-aprz: GITHUB_TOKEN is not set and no token could be obtained from the gh CLI.'
            Write-Warning 'cargo-aprz will use the unauthenticated GitHub API (60 requests/hour) and may fail on a full run.'
            Write-Warning 'To fix: run `gh auth login` (recommended), or set $env:GITHUB_TOKEN to a GitHub token, then re-run.'
        }
    }
    cargo aprz deps --error-if-high-risk --console appraisal
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[group("anvil-setup")]
anvil-aprz-setup installer="install": (anvil-tool-cargo-aprz-install installer)

[group("anvil-setup")]
anvil-aprz-validate-prereqs: anvil-tool-cargo-aprz-validate-prereqs
