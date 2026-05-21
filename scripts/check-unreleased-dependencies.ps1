# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Detects workspace crates that were modified vs the PR base ref but are not being
    released, when at least one of their downstream dependents *is* being released.

.DESCRIPTION
    Companion CI check to the interactive analysis performed by `release-crate.ps1`.
    Walks the transitive dependency graph forward from every crate whose version was
    bumped in this PR. For any upstream workspace dep that has file changes and is
    *not* itself being released, it emits a finding so reviewers can verify that the
    change is immaterial (formatting, doc tweaks) — or that the dep should have been
    released too.

    Writes a markdown comment to `-OutputFile` when findings exist, and sets the
    GitHub Actions step output `has_findings` to 'true' or 'false'. Exits 0 in both
    cases — this check is informational only and never fails the build.

.PARAMETER BaseRef
    The git ref to diff against. Defaults to 'origin/main'.

.PARAMETER OutputFile
    Path to the markdown file written when findings are non-empty. The file is only
    written when there is something to report.

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

    $releaseSet = @(Get-CratesWithVersionBumps -RepoRoot $repoRoot -BaseRef $BaseRef) | Sort-Object

    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add('## 📦 Unreleased Upstream Dependency Changes') | Out-Null
    $lines.Add('') | Out-Null
    $lines.Add('This PR releases the following workspace crate(s):') | Out-Null
    $lines.Add('') | Out-Null
    foreach ($f in $releaseSet) {
        $lines.Add((Format-ReleaseEntry -RepoRoot $repoRoot -BaseRef $BaseRef -Folder $f)) | Out-Null
    }
    $lines.Add('') | Out-Null
    $lines.Add('The following workspace crates were **modified** vs the base branch but are *not* part of this release:') | Out-Null
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
