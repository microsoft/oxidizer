# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Flags for reviewer attention any workspace packages with unreleased
    modifications that are transitively pulled in by a package this PR is
    releasing. Findings are advisory only — the author may have deliberately
    skipped releasing those packages because the modifications are immaterial.

.DESCRIPTION
    Companion CI check to the interactive analysis performed by `release-packages.ps1`.

    The "release set" is computed from `$BaseRef`: every package whose `version =`
    in `Cargo.toml` differs between `$BaseRef` and HEAD. For each package in that
    set, the script walks the transitive normal/build workspace dependency graph
    forward.

    "Modified" is evaluated **per package**, not against `$BaseRef`. For every
    workspace dependency, the baseline is the most recent commit that touched its own
    top-level `version =` or `publish =` line in its `Cargo.toml`. Any change
    under `crates/<dep>/` newer than that commit — committed (including merges
    from earlier PRs that didn't change the dep's version), working-tree, or
    untracked — is considered unreleased. This catches modifications that
    landed on `main` in a previous PR without a version increment and are now
    being depended on for the first time.

    Findings are emitted so reviewers can verify each change is immaterial
    (formatting, doc tweaks) — or that the dep should have been released too.

    Writes a markdown comment to `-OutputFile` when findings exist, and sets the
    GitHub Actions step output `has_findings` to 'true' or 'false'. Exits 0 in
    both cases — this check is informational only and never fails the build.

    The implementation lives in `scripts/lib/check-unreleased-deps.ps1` so the
    helpers can be unit-tested without invoking the main CLI flow.

.PARAMETER BaseRef
    The git ref used to identify the *release set* (packages whose `version =`
    differs between this ref and HEAD). It is **not** used as the modification
    baseline — that is computed per package from each package's own `Cargo.toml`
    history. Defaults to 'origin/main'.

.PARAMETER OutputFile
    Path to the markdown file written when findings are non-empty. The file is
    only written when there is something to report.

.EXAMPLE
    pwsh ./scripts/check-unreleased-dependencies.ps1 `
        -BaseRef "origin/$env:GITHUB_BASE_REF" `
        -OutputFile release-deps-comment.md
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$BaseRef = 'origin/main',

    [Parameter(Mandatory = $true)]
    [string]$OutputFile
)

. "$PSScriptRoot/lib/check-unreleased-deps.ps1"

Invoke-CheckUnreleasedDependencies -BaseRef $BaseRef -OutputFile $OutputFile
exit 0
