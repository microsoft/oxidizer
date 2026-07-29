# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Reports SemVer validation for each crate a PR is publishing, using
    cargo-semver-checks for ordinary libraries and explicit manual-review
    warnings for proc-macro-only crates.

.DESCRIPTION
    For every crate whose `[package] version` differs from the PR base ref (the
    "publishing set"), this script:

      1. reads the on-disk (this-PR) version,
      2. locates the crate's PREVIOUS version-bump commit — the most recent commit
         reachable from -BaseRef (so this PR's own bump, which lives only on the PR
         head, is excluded) that changed the crate's `[package] version` line,
      3. detects proc-macro-only crates from `cargo metadata`; those require
         explicit manual SemVer review because cargo-semver-checks has no
         supported API surface for them,
      4. for ordinary library crates, runs
         `cargo semver-checks --package <crate> --baseline-rev <sha>` so the
         comparison source is the crate's own source at that commit (the baseline
         rustdoc is rebuilt from git — no registry access, works OSS + enterprise),
      5. parses the required change type from the output, and
      6. computes the *minimum* version the increment should reach given the
         detected API changes,
      7. when a manually reviewed proc-macro-chain release has a breaking
         increment, marks its direct published consumers for manual review while
         preserving their automated result, and repeats only through consumers
         whose own increment is breaking.

    It writes a Markdown report to -ReportPath containing:
      - a summary status line (🛑 when at least one crate is under-incremented,
        ⚠️ when manual proc-macro review is required or a baseline could not
        be determined,
        ✅ when every publishing crate is sufficiently incremented),
      - a table: Crate | Baseline | Baseline commit | This PR | Minimum required | Status,
      - collapsible per-crate detail for under-incremented crates, proc-macro
        manual reviews, missing direct consumers, and crates whose baseline
        could not be determined, and
      - a link to the triggering Actions run.

    Two GitHub Actions step outputs are written to -GitHubOutput:
      publishing = 'true' | 'false'
      status     = 'pass' | 'warn' | 'fail'
                     fail = at least one crate is under-incremented;
                     warn = no crate is under-incremented but at least one
                            proc-macro needs manual review or a baseline could
                            not be determined (check incomplete);
                     pass = every crate is sufficiently incremented.
      A failed/unknown baseline or manual-review requirement is NEVER reported
      as 'fail' on its own — 'fail' is reserved for genuine under-increments.

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
# A row per crate: cargo name/folder, on-disk (this-PR) version, git-history
# baseline, the parsed required change type, whether the actual increment is
# breaking, the computed minimum version, manual-review provenance, and raw tool
# detail.
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
            $detail = "Baseline lookup failed — could not locate the crate's previous version-bump commit in git history. The semver comparison was skipped for this crate; verify the version increment manually.`n`n$($_.Exception.Message)"
            if ($pkg.IsProcMacroOnly) {
                $detail += "`n`nThis is also a proc-macro-only crate, so its procedural macro contract requires manual review. Because CI cannot determine whether its increment is breaking, direct published consumers are conservatively included in the manual review chain."
            }
            $rows.Add([pscustomobject]@{
                Folder      = $folder
                Crate       = $cargoName
                Baseline    = '⚠️ unknown'
                BaselineSha = ''
                OnDisk      = $onDisk
                Required    = '?'
                Sufficient  = $false
                ChangeType  = 'unknown'
                ReleaseIsBreaking           = $null
                RequiresManualSemverReview  = [bool]$pkg.IsProcMacroOnly
                ManualSemverReviewSources   = @()
                Detail      = $detail
            })
            continue
        }

        if ($pkg.IsProcMacroOnly) {
            $baselineVersion = if ($null -eq $bump) { 'new crate' } else { $bump.Version }
            $baselineSha = if ($null -eq $bump) { '' } else { $bump.Sha }
            $releaseIsBreaking = if ($null -eq $bump) {
                $false
            } else {
                $releaseChangeType = Get-ChangeTypeFromVersions -oldVersion $baselineVersion -newVersion $onDisk
                Test-IsBreakingChange -oldVersion $baselineVersion -ChangeType $releaseChangeType
            }

            Write-Host "cargo semver-checks: $cargoName — proc-macro-only target; manual SemVer review required."
            $rows.Add([pscustomobject]@{
                Folder      = $folder
                Crate       = $cargoName
                Baseline    = $baselineVersion
                BaselineSha = $baselineSha
                OnDisk      = $onDisk
                Required    = '?'
                Sufficient  = $null
                ChangeType  = 'manual'
                ReleaseIsBreaking          = $releaseIsBreaking
                RequiresManualSemverReview = $true
                ManualSemverReviewSources  = @()
                Detail      = "``$cargoName`` is a proc-macro-only crate. cargo-semver-checks intentionally skips proc-macro targets because they have no supported library API surface. Review exported macro names, accepted input syntax, diagnostics, and generated output manually; build and test results do not establish public API SemVer compatibility."
            })
            continue
        }

        if ($null -eq $bump) {
            # No prior version-bump commit reachable from BaseRef: a brand-new
            # crate (or one with no committed version history) — there is no
            # baseline to compare against, so nothing to enforce.
            Write-Host "cargo semver-checks: $cargoName (on-disk v$onDisk) — no prior version-bump commit, skipping."
            $rows.Add([pscustomobject]@{
                Folder      = $folder
                Crate       = $cargoName
                Baseline    = 'new crate'
                BaselineSha = ''
                OnDisk      = $onDisk
                Required    = $onDisk
                Sufficient  = $true
                ChangeType  = 'none'
                ReleaseIsBreaking          = $false
                RequiresManualSemverReview = $false
                ManualSemverReviewSources  = @()
                Detail      = ''
            })
            continue
        }

        $baselineVersion = $bump.Version
        $baselineSha     = $bump.Sha
        $shortSha        = if ($baselineSha.Length -ge 7) { $baselineSha.Substring(0, 7) } else { $baselineSha }

        $releaseChangeType = Get-ChangeTypeFromVersions -oldVersion $baselineVersion -newVersion $onDisk
        $releaseIsBreaking = Test-IsBreakingChange -oldVersion $baselineVersion -ChangeType $releaseChangeType

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
                Folder      = $folder
                Crate       = $cargoName
                Baseline    = "⚠️ $baselineVersion"
                BaselineSha = $baselineSha
                OnDisk      = $onDisk
                Required    = '?'
                Sufficient  = $false
                ChangeType  = 'unknown'
                ReleaseIsBreaking          = $releaseIsBreaking
                RequiresManualSemverReview = $false
                ManualSemverReviewSources  = @()
                Detail      = "cargo semver-checks could not be evaluated against ``$baselineSha`` (v$baselineVersion). The version increment was NOT verified — check it manually.`n`n$($_.Exception.Message)"
            })
            continue
        }

        # Determine the minimum acceptable version and whether the on-disk version
        # meets it. 'none' means there is no baseline (new crate) — no constraint.
        # Otherwise the minimum is the baseline incremented by the required change
        # type, and the crate is sufficient when its on-disk version is >= that
        # minimum. Comparing on-disk against the minimum (rather than trusting the
        # verdict alone) means a correctly-bumped crate — e.g. baseline 1.0.0 ->
        # on-disk 1.1.0 with a 'non-breaking' verdict (min 1.1.0) — is reported as
        # sufficient instead of being flagged for a bump it already has.
        if ($changeType -eq 'none') {
            $required   = $onDisk
            $sufficient = $true
        } else {
            $required   = Get-NextVersion -currentVersion $baselineVersion -ChangeType $changeType
            $sufficient = (Compare-SemanticVersions -version1 $onDisk -version2 $required) -ge 0
        }

        # Extract just the failure blocks for the collapsible detail.
        $detail = ($out -split "`n" |
            Where-Object { $_ -notmatch '^\s*(Cloning|Building|Built|Parsing|Parsed|Checking|Checked|Finished|Summary)\b' } |
            ForEach-Object { $_.TrimEnd() }) -join "`n"
        $detail = $detail.Trim()

        $rows.Add([pscustomobject]@{
            Folder      = $folder
            Crate       = $cargoName
            Baseline    = $baselineVersion
            BaselineSha = $baselineSha
            OnDisk      = $onDisk
            Required    = $required
            Sufficient  = $sufficient
            ChangeType  = $changeType
            ReleaseIsBreaking          = $releaseIsBreaking
            RequiresManualSemverReview = $false
            ManualSemverReviewSources  = @()
            Detail      = $detail
        })
    }
} finally {
    Pop-Location
}

# --- 3. Propagate manual review one breaking edge at a time. ------------------
$rowByFolder = @{}
foreach ($row in $rows) { $rowByFolder[$row.Folder] = $row }

$reviewQueue = [System.Collections.Generic.Queue[string]]::new()
# A known non-breaking increment stops propagation. A known breaking increment
# continues it. Unknown is conservative: CI cannot prove that the chain may stop,
# so it continues the warning to the next direct published consumer.
foreach ($row in $rows | Where-Object {
    $_.RequiresManualSemverReview -and $false -ne $_.ReleaseIsBreaking
}) {
    $reviewQueue.Enqueue($row.Folder)
}
$expandedReviewSources = [System.Collections.Generic.HashSet[string]]::new()
$missingManualReviews = New-Object 'System.Collections.Generic.List[object]'
$missingReviewKeys = [System.Collections.Generic.HashSet[string]]::new()

while ($reviewQueue.Count -gt 0) {
    $sourceFolder = $reviewQueue.Dequeue()
    if (-not $expandedReviewSources.Add($sourceFolder)) { continue }
    $sourceRow = $rowByFolder[$sourceFolder]
    if ($null -eq $sourceRow) { continue }

    $sourcePkg = $byFolder[$sourceFolder]
    $sourceCargoName = $sourcePkg.Name.Replace('-', '_')
    $directDependents = Get-DirectPublishedDependentsFromBaseline `
        -Baseline $packages `
        -TargetCargoName $sourceCargoName

    foreach ($dependentFolder in $directDependents) {
        if (-not $rowByFolder.ContainsKey($dependentFolder)) {
            $missingKey = "$sourceFolder->$dependentFolder"
            if ($missingReviewKeys.Add($missingKey)) {
                $missingManualReviews.Add([pscustomobject]@{
                    SourceFolder    = $sourceFolder
                    SourceCrate     = $sourceRow.Crate
                    DependentFolder = $dependentFolder
                    DependentCrate  = $byFolder[$dependentFolder].Name
                })
            }
            continue
        }

        $dependentRow = $rowByFolder[$dependentFolder]
        $dependentRow.RequiresManualSemverReview = $true
        $dependentRow.ManualSemverReviewSources = @(
            @($dependentRow.ManualSemverReviewSources) + $sourceRow.Crate |
                Sort-Object -Unique
        )

        if ($false -ne $dependentRow.ReleaseIsBreaking) {
            $reviewQueue.Enqueue($dependentFolder)
        }
    }
}

foreach ($row in $rows | Where-Object { @($_.ManualSemverReviewSources).Count -gt 0 }) {
    $sources = @($row.ManualSemverReviewSources) -join ', '
    $manualDetail = "Manual review is also required because direct dependency release(s) ``$sources`` were manually reviewed as breaking in the proc-macro compatibility chain. cargo-semver-checks still ran for this ordinary library when possible, but cannot prove whether the procedural macro contract is re-exported or otherwise exposed. If this crate's release is breaking, review continues to its direct published consumers; otherwise propagation stops here."
    if ([string]::IsNullOrWhiteSpace([string]$row.Detail)) {
        $row.Detail = $manualDetail
    } else {
        $row.Detail = "$($row.Detail)`n`n$manualDetail"
    }
}

# --- 4. Render the Markdown report. -------------------------------------------
$unknownRows = @($rows | Where-Object { $_.ChangeType -eq 'unknown' })
$manualRows  = @($rows | Where-Object { $_.RequiresManualSemverReview })
$realUnder   = @($rows | Where-Object {
    $_.ChangeType -notin @('unknown', 'manual') -and -not $_.Sufficient
})
$hasReal      = $realUnder.Count -gt 0
$hasUnknown   = $unknownRows.Count -gt 0
$hasManual    = $manualRows.Count -gt 0 -or $missingManualReviews.Count -gt 0
$anyProblem   = $hasReal -or $hasUnknown -or $hasManual

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
    if ($hasManual) {
        [void]$sb.AppendLine()
        [void]$sb.AppendLine("⚠️ **$($manualRows.Count)** publishing crate(s) require manual proc-macro compatibility review. Ordinary libraries retain their cargo-semver-checks result; review propagation advances only across a breaking release.")
    }
} elseif ($hasUnknown) {
    # No crate is under-incremented; the only problem is an unresolved baseline.
    # This is a warning (the check is incomplete), NOT an under-increment failure.
    [void]$sb.AppendLine('## ⚠️ Semver baseline could not be determined')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} could not determine the previous version-bump commit for **$($unknownRows.Count)** of $($rows.Count) crate(s), so their version increment was **not** verified. No crate was found to be under-incremented; check the crate(s) below manually.")
    if ($hasManual) {
        [void]$sb.AppendLine()
        [void]$sb.AppendLine("Additionally, **$($manualRows.Count)** publishing crate(s) require manual proc-macro compatibility review.")
    }
} elseif ($hasManual) {
    [void]$sb.AppendLine('## ⚠️ Manual proc-macro SemVer review required')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} intentionally does not analyse proc-macro-only targets. **$($manualRows.Count) of $($rows.Count)** publishing crate(s) require manual review either for their own procedural macro contract or because they directly consume a manually reviewed breaking release. Ordinary libraries retain their automated result, and review advances to the next dependency edge only when the current release is breaking.")
} else {
    [void]$sb.AppendLine('## ✅ Version increments look sufficient')
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("${bt}cargo semver-checks${bt} compared the **$($rows.Count)** crate(s) this PR publishes against their previous version-bump commit in git history. Every version increment is sufficient for the detected API changes.")
}
[void]$sb.AppendLine()
# Derive a commit base URL from the Actions run URL (…/<owner>/<repo>/actions/runs/<id>)
# so each baseline commit SHA links to the exact commit semver-checks ran against.
# Falls back to a bare code-formatted short SHA when the URL can't be parsed.
$commitBaseUrl = ''
if (-not [string]::IsNullOrEmpty($RunUrl)) {
    $m = [regex]::Match($RunUrl, '^(?<base>https?://[^\s]+?)/actions/runs/')
    if ($m.Success) { $commitBaseUrl = "$($m.Groups['base'].Value)/commit" }
}
[void]$sb.AppendLine('| Crate | Baseline | Baseline commit | This PR | Minimum required | Status |')
[void]$sb.AppendLine('|---|---|---|---|---|---|')
foreach ($r in $rows) {
    $requiresPropagatedReview = @($r.ManualSemverReviewSources).Count -gt 0
    if ($r.ChangeType -eq 'unknown' -and $r.RequiresManualSemverReview) {
        $status = '⚠️ baseline unknown; manual proc-macro chain review required'
        $req    = '—'
    } elseif ($r.ChangeType -eq 'unknown') {
        $status = '⚠️ baseline unknown — not verified'
        $req    = '—'
    } elseif ($r.ChangeType -eq 'manual') {
        $status = '⚠️ manual proc-macro review required'
        $req    = '—'
    } elseif (-not $r.Sufficient) {
        $status = "🛑 increase to at least ${bt}$($r.Required)${bt}"
        if ($requiresPropagatedReview) { $status += '; manual chain review required' }
        $req    = "**$($r.Required)**"
    } elseif ($requiresPropagatedReview) {
        $status = '⚠️ automated check ok; manual proc-macro chain review required'
        $req    = $r.Required
    } elseif ($r.Sufficient) {
        $status = '✅ ok'
        $req    = $r.Required
    }

    # The resolved commit semver-checks compared against (--baseline-rev), shown
    # per crate. Empty for new crates and failed baseline lookups (no commit).
    if ([string]::IsNullOrEmpty($r.BaselineSha)) {
        $commitCell = '—'
    } else {
        $short = if ($r.BaselineSha.Length -ge 7) { $r.BaselineSha.Substring(0, 7) } else { $r.BaselineSha }
        if ($commitBaseUrl) {
            $commitCell = "[${bt}$short${bt}]($commitBaseUrl/$($r.BaselineSha))"
        } else {
            $commitCell = "${bt}$short${bt}"
        }
    }

    [void]$sb.AppendLine("| ${bt}$($r.Crate)${bt} | $($r.Baseline) | $commitCell | $($r.OnDisk) | $req | $status |")
}
[void]$sb.AppendLine()

$fence = "$bt$bt$bt"
if ($anyProblem) {
    foreach ($r in $rows | Where-Object {
        $_.RequiresManualSemverReview -or
        $_.ChangeType -in @('unknown', 'manual') -or
        -not $_.Sufficient
    }) {
        $isRealUnder = $r.ChangeType -notin @('unknown', 'manual') -and -not $r.Sufficient
        $icon = if ($isRealUnder) { '🛑' } else { '⚠️' }
        $summary = if (@($r.ManualSemverReviewSources).Count -gt 0) {
            'manual proc-macro propagation review detail'
        } else {
            switch ($r.ChangeType) {
                'unknown' { 'baseline lookup detail' }
                'manual'  { 'manual proc-macro review detail' }
                default   { 'cargo semver-checks detail' }
            }
        }
        [void]$sb.AppendLine("<details><summary>$icon <code>$($r.Crate)</code> — $summary</summary>")
        [void]$sb.AppendLine()
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine($r.Detail)
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine('</details>')
        [void]$sb.AppendLine()
    }
    foreach ($missing in $missingManualReviews) {
        [void]$sb.AppendLine("<details><summary>⚠️ <code>$($missing.DependentCrate)</code> — missing direct-consumer review</summary>")
        [void]$sb.AppendLine()
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine("``$($missing.SourceCrate)`` has a breaking release in the manual proc-macro compatibility chain, but direct published consumer ``$($missing.DependentCrate)`` is not in this PR's publishing set. Run the release planner so the consumer receives its patch cascade floor, cargo-semver-checks analysis, and mandatory manual review.")
        [void]$sb.AppendLine($fence)
        [void]$sb.AppendLine('</details>')
        [void]$sb.AppendLine()
    }
    if ($hasReal) {
        [void]$sb.AppendLine('> If these breaking changes are intentional, increase each crate to at least its **Minimum required** version. This check is **informational and does not block the merge**.')
    } elseif ($hasUnknown) {
        [void]$sb.AppendLine('> The baseline could not be determined for the crate(s) above, so their version increments were not verified. This check is **informational and does not block the merge**.')
    } else {
        [void]$sb.AppendLine('> Proc-macro API compatibility must be reviewed manually; successful builds and tests do not establish SemVer compatibility. This check is **informational and does not block the merge**.')
    }
    if ($missingManualReviews.Count -gt 0) {
        [void]$sb.AppendLine()
        [void]$sb.AppendLine('> At least one direct published consumer that requires review is absent from the publishing set. Re-run the release planner before merging.')
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
$status = if ($hasReal) { 'fail' } elseif ($hasUnknown -or $hasManual) { 'warn' } else { 'pass' }
Write-Outputs -publishing 'true' -status $status
