# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Runs cargo-semver-checks for each crate a PR is publishing and renders a
    rich, per-crate Markdown report comparing the on-disk version increment
    against the minimum increment the crate's API changes require versus the
    crate's previous version-bump commit in git history.

.DESCRIPTION
    For every crate whose `[package] version` differs from the PR base ref (the
    "publishing set"), this script:

      1. reads the on-disk (this-PR) version,
      2. locates the crate's PREVIOUS version-bump commit — the most recent commit
         reachable from -BaseRef (so this PR's own bump, which lives only on the PR
         head, is excluded) that changed the crate's `[package] version` line,
      3. runs `cargo semver-checks --package <crate> --baseline-rev <sha>` so the
         comparison source is the crate's own source at that commit (the baseline
         rustdoc is rebuilt from git — no registry access, works OSS + enterprise),
      4. parses the required change type from the output, and
      5. computes the *minimum* version the increment should reach given the
         detected API changes.

    It writes a Markdown report to -ReportPath containing:
      - a summary status line (🛑 when at least one crate is under-incremented,
        ⚠️ when the only problem is a baseline that could not be determined,
        ✅ when every publishing crate is sufficiently incremented),
      - a table: Crate | Baseline | This PR | Minimum required | Status,
      - collapsible per-crate detail for under-incremented crates and for
        crates whose baseline could not be determined, and
      - a link to the triggering Actions run.

    Two GitHub Actions step outputs are written to -GitHubOutput:
      publishing = 'true' | 'false'
      status     = 'pass' | 'warn' | 'fail'
                     fail = at least one crate is under-incremented;
                     warn = no crate is under-incremented but at least one
                            baseline could not be determined (check incomplete);
                     pass = every crate is sufficiently incremented.
      A failed/unknown baseline is NEVER reported as 'fail' on its own — 'fail'
      is reserved for genuine under-increments per this contract.

    The report is informational: callers keep the job non-failing.

.PARAMETER BaseRef
    Git ref to diff against, e.g. 'origin/main'. Must be fetched beforehand.
    Also the ref the previous version-bump commit is searched from, so this PR's
    own version bump is excluded from the baseline.

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
# Get-PackagesWithVersionChanges returns a HashSet via Write-Output -NoEnumerate
# (so its internal callers can use .Contains()). Casting to [string[]] reliably
# enumerates that set into a flat array — do NOT wrap the raw return in @(...),
# which produces a 1-element array containing the (possibly empty) HashSet and
# makes the "nothing to publish" guard below never fire (leading to a spurious
# "0 crate(s)" comment on non-publishing PRs).
$changedFolders = @([string[]](Get-PackagesWithVersionChanges -RepoRoot $RepoRoot -BaseRef $BaseRef) | Sort-Object)
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
# A row per crate: cargo name, on-disk (this-PR) version, git-history baseline,
# the parsed required change type, the computed minimum version, and the raw
# tool detail (for under-incremented crates).
$rows = New-Object System.Collections.Generic.List[object]

Push-Location $RepoRoot
try {
    foreach ($folder in $changedFolders) {
        $pkg = $byFolder[$folder]
        if ($null -eq $pkg) { continue }

        $cargoName    = $pkg.Name
        $onDisk       = Get-CurrentVersion -cargoTomlPath (Join-Path $RepoRoot "crates/$folder/Cargo.toml")

        # Locate the crate's previous version-bump commit reachable from BaseRef
        # (so this PR's own bump is excluded). Its declared [package] version is
        # the baseline number; its SHA is the semver-checks --baseline-rev.
        # Locating the commit inspects git history only — no registry access.
        try {
            $bump = Get-PreviousVersionBumpCommit -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $folder
        } catch {
            Write-Host "cargo semver-checks: $cargoName (on-disk v$onDisk) — baseline lookup FAILED: $($_.Exception.Message)"
            $rows.Add([pscustomobject]@{
                Crate       = $cargoName
                Baseline    = '⚠️ unknown'
                OnDisk      = $onDisk
                Required    = '?'
                Sufficient  = $false
                ChangeType  = 'unknown'
                Detail      = "Baseline lookup failed — could not locate the crate's previous version-bump commit in git history. The semver comparison was skipped for this crate; verify the version increment manually.`n`n$($_.Exception.Message)"
            })
            continue
        }

        if ($null -eq $bump) {
            # No prior version-bump commit reachable from BaseRef: a brand-new
            # crate (or one with no committed version history) — there is no
            # baseline to compare against, so nothing to enforce.
            Write-Host "cargo semver-checks: $cargoName (on-disk v$onDisk) — no prior version-bump commit, skipping."
            $rows.Add([pscustomobject]@{
                Crate       = $cargoName
                Baseline    = 'new crate'
                OnDisk      = $onDisk
                Required    = $onDisk
                Sufficient  = $true
                ChangeType  = 'none'
                Detail      = ''
            })
            continue
        }

        $baselineVersion = $bump.Version
        $baselineSha     = $bump.Sha
        $shortSha        = if ($baselineSha.Length -ge 7) { $baselineSha.Substring(0, 7) } else { $baselineSha }

        Write-Host "cargo semver-checks: $cargoName (on-disk v$onDisk) vs v$baselineVersion @ $shortSha..."
        $PSNativeCommandUseErrorActionPreference = $false
        $out = & cargo semver-checks --package $cargoName --baseline-rev $baselineSha --all-features --color never 2>&1 | Out-String

        # A build/tool failure makes ConvertFrom-SemverChecksOutput throw (no
        # silent fallback); surface that as a ⚠️ unknown row rather than failing
        # the whole report or misreporting the crate as sufficient.
        try {
            $changeType = ConvertFrom-SemverChecksOutput -Output $out -ExitCode $LASTEXITCODE -PackageName $cargoName
        } catch {
            Write-Host "cargo semver-checks: $cargoName — analysis FAILED: $($_.Exception.Message)"
            $rows.Add([pscustomobject]@{
                Crate       = $cargoName
                Baseline    = "⚠️ $baselineVersion ($shortSha)"
                OnDisk      = $onDisk
                Required    = '?'
                Sufficient  = $false
                ChangeType  = 'unknown'
                Detail      = "cargo semver-checks could not be evaluated against ``$baselineSha`` (v$baselineVersion). The version increment was NOT verified — check it manually.`n`n$($_.Exception.Message)"
            })
            continue
        }

        # A 'breaking'/'non-breaking' verdict means the detected API changes need
        # a stronger increment than the on-disk version gives over the baseline;
        # the minimum acceptable version is the baseline incremented by that
        # change type. 'patch' means the on-disk version already covers the
        # changes; 'none' means the crate is new (no baseline to violate).
        $sufficient = $changeType -in @('patch', 'none')
        if ($sufficient) {
            $required = $onDisk
        } else {
            $required = Get-NextVersion -currentVersion $baselineVersion -ChangeType $changeType
        }

        # Extract just the failure blocks for the collapsible detail.
        $detail = ($out -split "`n" |
            Where-Object { $_ -notmatch '^\s*(Cloning|Building|Built|Parsing|Parsed|Checking|Checked|Finished|Summary)\b' } |
            ForEach-Object { $_.TrimEnd() }) -join "`n"
        $detail = $detail.Trim()

        $rows.Add([pscustomobject]@{
            Crate       = $cargoName
            Baseline    = "$baselineVersion ($shortSha)"
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
$unknownRows = @($rows | Where-Object { $_.ChangeType -eq 'unknown' })
$realUnder   = @($underBumped | Where-Object { $_.ChangeType -ne 'unknown' })
$hasReal      = $realUnder.Count -gt 0
$hasUnknown   = $unknownRows.Count -gt 0
$anyProblem   = $underBumped.Count -gt 0

$bt = [char]0x60   # backtick, kept in a variable to avoid PowerShell escaping.
$sb = New-Object System.Text.StringBuilder
if ($hasReal) {
    # At least one crate is genuinely under-incremented — the real failure case.
    [void]$sb.AppendLine('## 🛑 Additional version increments required')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} compared the crate(s) this PR publishes against their previous version-bump commit in git history. **$($realUnder.Count) of $($rows.Count)** need a higher version than this PR sets — the increment already applied is not enough for the API changes:")
    if ($hasUnknown) {
        [void]$sb.AppendLine()
        [void]$sb.AppendLine("⚠️ The baseline (previous version-bump commit) could not be determined for **$($unknownRows.Count)** other crate(s); their version increment was **not** verified — check them manually.")
    }
} elseif ($hasUnknown) {
    # No crate is under-incremented; the only problem is an unresolved baseline.
    # This is a warning (the check is incomplete), NOT an under-increment failure.
    [void]$sb.AppendLine('## ⚠️ Semver baseline could not be determined')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} could not determine the previous version-bump commit for **$($unknownRows.Count)** of $($rows.Count) crate(s), so their version increment was **not** verified. No crate was found to be under-incremented; check the crate(s) below manually.")
} else {
    [void]$sb.AppendLine('## ✅ Version increments look sufficient')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} compared the **$($rows.Count)** crate(s) this PR publishes against their previous version-bump commit in git history. Every version increment is sufficient for the detected API changes.")
}
[void]$sb.AppendLine()
[void]$sb.AppendLine('| Crate | Baseline | This PR | Minimum required | Status |')
[void]$sb.AppendLine('|---|---|---|---|---|')
foreach ($r in $rows) {
    if ($r.ChangeType -eq 'unknown') {
        $status = '⚠️ baseline unknown — not verified'
        $req    = '—'
    } elseif ($r.Sufficient) {
        $status = '✅ ok'
        $req    = $r.Required
    } else {
        $status = "🛑 increase to at least ${bt}$($r.Required)${bt}"
        $req    = "**$($r.Required)**"
    }
    [void]$sb.AppendLine("| ${bt}$($r.Crate)${bt} | $($r.Baseline) | $($r.OnDisk) | $req | $status |")
}
[void]$sb.AppendLine()

$fence = "$bt$bt$bt"
if ($anyProblem) {
    foreach ($r in $underBumped) {
        $icon    = if ($r.ChangeType -eq 'unknown') { '⚠️' } else { '🛑' }
        $summary = if ($r.ChangeType -eq 'unknown') { 'baseline lookup detail' } else { 'cargo semver-checks detail' }
        [void]$sb.AppendLine("<details><summary>$icon <code>$($r.Crate)</code> — $summary</summary>")
        [void]$sb.AppendLine()
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine($r.Detail)
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine('</details>')
        [void]$sb.AppendLine()
    }
    if ($hasReal) {
        [void]$sb.AppendLine('> If these breaking changes are intentional, increase each crate to at least its **Minimum required** version. This check is **informational and does not block the merge**.')
    } else {
        [void]$sb.AppendLine('> The baseline could not be determined for the crate(s) above, so their version increments were not verified. This check is **informational and does not block the merge**.')
    }
} else {
    [void]$sb.AppendLine('> This check is informational and does not block the merge.')
}

if (-not [string]::IsNullOrEmpty($RunUrl)) {
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("[View the check run]($RunUrl)")
}

Set-Content -Path $ReportPath -Value $sb.ToString() -Encoding utf8
Write-Host "Report written to $ReportPath"
$status = if ($hasReal) { 'fail' } elseif ($hasUnknown) { 'warn' } else { 'pass' }
Write-Outputs -publishing 'true' -status $status
