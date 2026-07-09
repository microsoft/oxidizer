# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Runs cargo-semver-checks for each crate a PR is publishing and renders a
    rich, per-crate Markdown report comparing the on-disk bump against the
    minimum bump the crate's API changes require versus its crates.io baseline.

.DESCRIPTION
    For every crate whose `[package] version` differs from the PR base ref (the
    "publishing set"), this script:

      1. reads the on-disk (this-PR) version,
      2. runs `cargo semver-checks --package <crate>` against the crate's last
         published crates.io release (the tool's default baseline),
      3. parses the baseline version and the required change type from the
         output, and
      4. computes the *minimum* version the bump should reach given the detected
         API changes.

    It writes a Markdown report to -ReportPath containing:
      - a summary status line (🛑 when at least one crate is under-bumped,
        ✅ when every publishing crate is sufficiently bumped),
      - a table: Crate | crates.io | This PR | Minimum required | Status,
      - collapsible per-crate `cargo semver-checks` detail for under-bumped
        crates, and
      - a link to the triggering Actions run.

    Two GitHub Actions step outputs are written to -GitHubOutput:
      publishing = 'true' | 'false'
      status     = 'pass' | 'fail'   (fail = at least one crate under-bumped)

    The report is informational: callers keep the job non-failing.

.PARAMETER BaseRef
    Git ref to diff against, e.g. 'origin/main'. Must be fetched beforehand.

.PARAMETER ReportPath
    Path to write the Markdown report to.

.PARAMETER RunUrl
    URL of the Actions run, embedded as a footer link. Optional.

.PARAMETER RepoRoot
    Repository root. Defaults to the current directory.

.PARAMETER GitHubOutput
    Path to the GitHub Actions step-output file. Defaults to $env:GITHUB_OUTPUT.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BaseRef,
    [Parameter(Mandatory = $true)][string]$ReportPath,
    [string]$RunUrl = '',
    [string]$RepoRoot = (Get-Location).Path,
    [string]$GitHubOutput = $env:GITHUB_OUTPUT
)

. "$PSScriptRoot/../lib/releasing.ps1"

# --- 1. Determine the publishing set (version-bumped published crates). -------
$changedFolders = @(Get-PackagesWithVersionChanges -RepoRoot $RepoRoot -BaseRef $BaseRef)
$packages = Get-WorkspacePackages -repoRoot $RepoRoot
$byFolder = @{}
foreach ($p in $packages) { $byFolder[$p.Folder] = $p }

function Write-Outputs([string]$publishing, [string]$status) {
    $lines = @("publishing=$publishing", "status=$status")
    if ([string]::IsNullOrEmpty($GitHubOutput)) {
        $lines | ForEach-Object { Write-Output $_ }
    } else {
        $lines | Add-Content -Path $GitHubOutput -Encoding utf8
    }
}

if ($changedFolders.Count -eq 0) {
    Write-Host 'No crate versions changed; nothing to publish.'
    Write-Outputs -publishing 'false' -status 'pass'
    return
}

# --- 2. Run cargo-semver-checks per crate and gather results. -----------------
# A row per crate: cargo name, on-disk (this-PR) version, crates.io baseline,
# the parsed required change type, the computed minimum version, and the raw
# tool detail (for under-bumped crates).
$rows = New-Object System.Collections.Generic.List[object]

Push-Location $RepoRoot
try {
    foreach ($folder in ($changedFolders | Sort-Object)) {
        $pkg = $byFolder[$folder]
        if ($null -eq $pkg) { continue }

        $cargoName    = $pkg.Name
        $onDisk       = Get-CurrentVersion -cargoTomlPath (Join-Path $RepoRoot "crates/$folder/Cargo.toml")

        Write-Host "cargo semver-checks: $cargoName (on-disk v$onDisk) vs crates.io..."
        $PSNativeCommandUseErrorActionPreference = $false
        $out = & cargo semver-checks --package $cargoName --all-features --color never 2>&1 | Out-String

        $changeType = ConvertFrom-SemverChecksOutput -Output $out -ExitCode $LASTEXITCODE -PackageName $cargoName

        # Baseline version cargo-semver-checks pulled from the registry, parsed
        # from a "Checking <name> v<baseline> -> v<current>" line. Absent for a
        # never-published crate (changeType 'none').
        $baseline = '—'
        $m = [regex]::Match($out, "(?im)^\s*Checking\s+\S+\s+v(\d+\.\d+\.\d+\S*)\s*->")
        if ($m.Success) { $baseline = $m.Groups[1].Value }

        # A 'breaking'/'non-breaking' verdict means the detected API changes need
        # a stronger bump than the on-disk version gives over the baseline; the
        # minimum acceptable version is baseline bumped by that change type.
        # 'patch' means the on-disk bump already covers the changes; 'none' means
        # the crate is new (no baseline to violate).
        $sufficient = $changeType -in @('patch', 'none')
        if ($sufficient) {
            $required = $onDisk
        } else {
            $required = Get-NextVersion -currentVersion $baseline -ChangeType $changeType
        }

        # Extract just the failure blocks for the collapsible detail.
        $detail = ($out -split "`n" |
            Where-Object { $_ -notmatch '^\s*(Cloning|Building|Built|Parsing|Parsed|Checking|Checked|Finished|Summary)\b' } |
            ForEach-Object { $_.TrimEnd() }) -join "`n"
        $detail = $detail.Trim()

        $rows.Add([pscustomobject]@{
            Crate       = $cargoName
            Baseline    = $baseline
            OnDisk      = $onDisk
            Required    = $required
            Sufficient  = $sufficient
            ChangeType  = $changeType
            Detail      = $detail
        })
    }
} finally {
    Pop-Location
}

# --- 3. Render the Markdown report. -------------------------------------------
$underBumped = @($rows | Where-Object { -not $_.Sufficient })
$overallFail = $underBumped.Count -gt 0

$bt = [char]0x60   # backtick, kept in a variable to avoid PowerShell escaping.
$sb = New-Object System.Text.StringBuilder
if ($overallFail) {
    [void]$sb.AppendLine('## 🛑 Semver: version bump required')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} compared the crate(s) this PR publishes against their latest **crates.io** release. **$($underBumped.Count) of $($rows.Count)** need a higher version bump than the one on disk:")
} else {
    [void]$sb.AppendLine('## ✅ Semver: version bumps look sufficient')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} compared the **$($rows.Count)** crate(s) this PR publishes against their latest **crates.io** release. Every bump is sufficient for the detected API changes.")
}
[void]$sb.AppendLine()
[void]$sb.AppendLine('| Crate | crates.io | This PR | Minimum required | Status |')
[void]$sb.AppendLine('|---|---|---|---|---|')
foreach ($r in $rows) {
    if ($r.Sufficient) {
        $status = '✅ ok'
        $req    = $r.Required
    } else {
        $status = "🛑 bump to at least ${bt}$($r.Required)${bt}"
        $req    = "**$($r.Required)**"
    }
    [void]$sb.AppendLine("| ${bt}$($r.Crate)${bt} | $($r.Baseline) | $($r.OnDisk) | $req | $status |")
}
[void]$sb.AppendLine()

$fence = "$bt$bt$bt"
if ($overallFail) {
    foreach ($r in $underBumped) {
        [void]$sb.AppendLine("<details><summary>🛑 <code>$($r.Crate)</code> — cargo semver-checks detail</summary>")
        [void]$sb.AppendLine()
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine($r.Detail)
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine('</details>')
        [void]$sb.AppendLine()
    }
    [void]$sb.AppendLine('> If these breaking changes are intentional, bump each crate to at least its **Minimum required** version. This check is **informational and does not block the merge**.')
} else {
    [void]$sb.AppendLine('> This check is informational and does not block the merge.')
}

if (-not [string]::IsNullOrEmpty($RunUrl)) {
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("[View the semver check run]($RunUrl)")
}

Set-Content -Path $ReportPath -Value $sb.ToString() -Encoding utf8
Write-Host "Report written to $ReportPath"
Write-Outputs -publishing 'true' -status ($(if ($overallFail) { 'fail' } else { 'pass' }))
