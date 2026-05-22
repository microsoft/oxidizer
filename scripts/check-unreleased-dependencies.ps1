# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Flags workspace crates with unreleased modifications that are transitively
    pulled in by a crate this PR is releasing.

.DESCRIPTION
    Companion CI check to the interactive analysis performed by `release-crate.ps1`.

    The "release set" is computed from `$BaseRef`: every crate whose `version =`
    in `Cargo.toml` differs between `$BaseRef` and HEAD. For each crate in that
    set, the script walks the transitive normal/build workspace dependency graph
    forward.

    "Modified" is evaluated **per crate**, not against `$BaseRef`. For every
    upstream dep, the baseline is the most recent commit that touched its own
    top-level `version =` or `publish =` line in its `Cargo.toml`. Any change
    under `crates/<dep>/` newer than that commit — committed (including merges
    from earlier PRs that didn't bump the dep), working-tree, or untracked — is
    considered unreleased. This catches modifications that landed on `main` in a
    previous PR without a version bump and are now being depended on for the
    first time.

    Findings are emitted so reviewers can verify each change is immaterial
    (formatting, doc tweaks) — or that the dep should have been released too.

    Writes a markdown comment to `-OutputFile` when findings exist, and sets the
    GitHub Actions step output `has_findings` to 'true' or 'false'. Exits 0 in
    both cases — this check is informational only and never fails the build.

.PARAMETER BaseRef
    The git ref used to identify the *release set* (crates whose `version =`
    differs between this ref and HEAD). It is **not** used as the modification
    baseline — that is computed per crate from each crate's own `Cargo.toml`
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

$ErrorActionPreference = 'Stop'

. "$PSScriptRoot/lib/releasing.ps1"

function Set-StepOutput {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Value
    )

    if ([string]::IsNullOrEmpty($env:GITHUB_OUTPUT)) { return }
    Add-Content -Path $env:GITHUB_OUTPUT -Value "$Name=$Value"
}

function Get-RepoRoot {
    $output = Invoke-Git -Arguments @('rev-parse', '--show-toplevel')
    return ($output | Select-Object -First 1).ToString().Trim()
}

function Format-DependencyChain {
    param([Parameter(Mandatory = $true)][string[]]$Chain)
    return ($Chain -join ' -> ')
}

function Format-ReleaseEntry {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef,
        [Parameter(Mandatory = $true)][string]$Folder
    )

    $cargo = Join-Path (Join-Path $RepoRoot 'crates') $Folder 'Cargo.toml'
    $current = if (Test-Path $cargo) { Get-CurrentVersion -cargoTomlPath $cargo } else { '?' }
    $base = Get-CrateVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -CrateFolder $Folder
    if ([string]::IsNullOrEmpty($base)) {
        return "  - ``$Folder`` $current (new crate)"
    }
    return "  - ``$Folder`` $base -> $current"
}

try {
    $repoRoot = Get-RepoRoot

    if (-not (Test-GitRef -Ref $BaseRef -RepoRoot $repoRoot)) {
        Write-Warning "Base ref '$BaseRef' could not be resolved; skipping analysis."
        Set-StepOutput -Name 'has_findings' -Value 'false'
        exit 0
    }

    $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $repoRoot -BaseRef $BaseRef)

    if ($findings.Count -eq 0) {
        Write-Host "No modified-but-unreleased upstream dependencies detected."
        Set-StepOutput -Name 'has_findings' -Value 'false'
        exit 0
    }

    # Get-CratesWithVersionBumps returns a HashSet via Write-Output -NoEnumerate so
    # callers can use .Contains() on it. That same wrapping defeats `Sort-Object`:
    # piping a NoEnumerate'd HashSet sends it as a single object, and the sort
    # becomes a no-op. Unwrap explicitly via ForEach-Object before sorting.
    $releaseSetHash = Get-CratesWithVersionBumps -RepoRoot $repoRoot -BaseRef $BaseRef
    $releaseSet = @($releaseSetHash | ForEach-Object { $_ }) | Sort-Object

    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add('## 📦 Unreleased Upstream Dependency Changes') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('This PR releases the following workspace crate(s):') | Out-Null
    $lines.Add('') | Out-Null
    foreach ($f in $releaseSet) {
        $lines.Add((Format-ReleaseEntry -RepoRoot $repoRoot -BaseRef $BaseRef -Folder $f)) | Out-Null
    }
    $lines.Add('') | Out-Null
    $lines.Add('The following workspace crates have **unreleased modifications** (changes newer than their last `version =` or `publish =` bump) and are *not* part of this release:') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('| Crate | Files changed | Reached via |') | Out-Null
    $lines.Add('|-------|--------------:|-------------|') | Out-Null

    $sortedFindings = $findings | Sort-Object { $_.Folder }
    foreach ($finding in $sortedFindings) {
        $chains = @($finding.DependencyChains | ForEach-Object { Format-DependencyChain -Chain $_ })
        $rendered = ($chains | Sort-Object -Unique | ForEach-Object { "``$_``" }) -join '<br>'
        $lines.Add("| ``$($finding.Folder)`` | $($finding.ChangedFileCount) | $rendered |") | Out-Null
    }

    $lines.Add('') | Out-Null
    $lines.Add('### What this means') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('Locally, the released crate(s) build against the modified version of each unreleased dependency via path-references. Once published, however, they will resolve against the **last released** version of each dependency on crates.io — which does not include the unreleased changes.') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('- If the unreleased changes are **material** to the released crate''s behavior or public API, you should release the dependency too (re-run ``scripts/release-crate.ps1`` for it).') | Out-Null
    $lines.Add('- If the changes are **immaterial** (formatting, doc tweaks, internal-only refactors), this comment can be ignored.') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('<sub>This is an automated informational check. It does not fail the build.</sub>') | Out-Null

    $content = ($lines -join "`n") + "`n"
    Set-Content -Path $OutputFile -Value $content -Encoding utf8 -NoNewline

    Write-Host "Wrote $($findings.Count) finding(s) to '$OutputFile'."
    Set-StepOutput -Name 'has_findings' -Value 'true'
    exit 0
}
catch {
    Write-Error "check-unreleased-dependencies.ps1 failed: $_"
    # Don't block the PR on tool failures. Surface has_findings=false so no stale
    # comment is left posted.
    Set-StepOutput -Name 'has_findings' -Value 'false'
    exit 0
}
