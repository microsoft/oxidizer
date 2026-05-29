# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Release-flow library: helpers and orchestration for scripts/release-crate.ps1.

.DESCRIPTION
    Owns the orchestration helpers, changelog formatters, and the Invoke-ReleaseMain
    entrypoint that drives the full crate-release workflow. scripts/release-crate.ps1
    is a thin CLI shell that dot-sources this library and calls Invoke-ReleaseMain.

    This file is NOT an entrypoint. It only defines functions and module-scoped
    configuration; dot-source it from another script (or from Pester tests) to
    consume its API.

    Depends on scripts/lib/releasing.ps1 (which it dot-sources at the top so
    consumers only need to source this file).
#>

# --- DOT-SOURCE SHARED LIBRARY ---
#
# scripts/lib/releasing.ps1 owns the lower-level reusable building blocks used by
# both the release flow below and scripts/check-unreleased-dependencies.ps1:
#   - Compiled regex patterns ($script:ConventionalCommitRegex, $script:PrReferenceRegex,
#     $script:SemanticVersionRegex, $script:CargoPackageVersionRegex, $script:GitHubRepoRegex,
#     $script:RegexEscapeRegex).
#   - Safe git invocation (Invoke-Git) and ref validation (Test-GitRef).
#   - SemVer arithmetic (Compare-SemanticVersions, Get-NextVersion, Get-BumpKindFromVersions,
#     Test-IsBreakingChange) and crate-version readers (Get-CurrentVersion,
#     Get-CrateVersionFromRef).
#   - Workspace metadata (Get-WorkspaceMetadata, Get-WorkspaceCrates,
#     Invalidate-WorkspaceMetadataCache, Test-CrateExposesTarget, Get-AllTransitiveDependents).
#   - Modified-but-unreleased dependency analysis (Get-CratesWithUnreleasedChanges,
#     Get-CratesWithVersionBumps, Get-UnreleasedModifiedDependencies).
. "$PSScriptRoot/releasing.ps1"

# --- CONFIGURATION ---

# Maps commit types (e.g., 'chore') to a common group key (e.g., 'task').
$script:TypeGroupMapping = @{
    'chore' = 'task';
    'doc'   = 'docs';
    'misc'  = 'miscellaneous';
}

# Maps the final group key to a user-friendly header in the changelog.
$script:HeaderNameMapping = @{
    'breaking'      = '⚠️ Breaking';
    'build'         = '🏗️ Build System';
    'ci'            = '🔄 Continuous Integration';
    'docs'          = '📚 Documentation';
    'feat'          = '✨ Features';
    'fix'           = '🐛 Bug Fixes';
    'miscellaneous' = '🧩 Miscellaneous';
    'perf'          = '⚡ Performance';
    'refactor'      = '♻️ Code Refactoring';
    'style'         = '🎨 Styling';
    'task'          = '✔️ Tasks';
}

# Defines the preferred order for commit type sections in the changelog.
$script:TypeOrder = @('breaking', 'feat', 'fix', 'perf', 'docs', 'task', 'refactor', 'build', 'ci', 'style')

# Defines commit types that should be excluded from the changelog.
$script:IgnoredTypes = @('test')

# --- HELPER FUNCTIONS ---

function Test-CommandExists {
    param([string]$Command)
    return $null -ne (Get-Command $Command -ErrorAction SilentlyContinue)
}

function Sort-KeysByPreferredOrder {
    param(
        [string[]]$allKeys,
        [string[]]$preferredOrder
    )
    $sortedKeys = [System.Collections.ArrayList]::new()
    $remainingKeys = [System.Collections.ArrayList]::new()
    $remainingKeys.AddRange($allKeys)

    foreach ($key in $preferredOrder) {
        if ($remainingKeys.Contains($key)) {
            $null = $sortedKeys.Add($key)
            $null = $remainingKeys.Remove($key)
        }
    }

    $remainingKeys.Sort()
    $sortedKeys.AddRange($remainingKeys)
    return $sortedKeys
}

function Format-ConventionalCommits {
    param(
        [string[]]$rawCommitMessages,
        [string]$prBaseUrl
    )

    if (-not $rawCommitMessages) {
        return @()
    }

    $groupedCommits = [ordered]@{}

    foreach ($message in $rawCommitMessages) {
        $type = "miscellaneous"
        $description = $message
        $isConventional = $false

        $conventionalMatch = $script:ConventionalCommitRegex.Match($message)
        $isBreaking = $false
        if ($conventionalMatch.Success) {
            $type = $conventionalMatch.Groups[1].Value
            $isBreaking = $conventionalMatch.Groups[2].Value -eq '!'
            $description = $conventionalMatch.Groups[3].Value
            $isConventional = $true
        }

        if ($isConventional -and $script:IgnoredTypes -contains $type) {
            continue
        }

        if (-not [string]::IsNullOrEmpty($prBaseUrl)) {
            $prMatch = $script:PrReferenceRegex.Match($description)
            if ($prMatch.Success) {
                $fullMatch = $prMatch.Groups[0].Value
                $prNumber  = $prMatch.Groups[2].Value
                $prLink    = " ([#$prNumber]($prBaseUrl/$prNumber))"
                $description = $description.Substring(0, $description.Length - $fullMatch.Length) + $prLink
            }
        }

        # Breaking changes are grouped separately, regardless of the commit type
        $groupKey = if ($isBreaking) {
            'breaking'
        } elseif ($script:TypeGroupMapping.ContainsKey($type)) {
            $script:TypeGroupMapping[$type]
        } else {
            $type
        }

        if (-not $groupedCommits.Contains($groupKey)) {
            $groupedCommits[$groupKey] = [System.Collections.ArrayList]::new()
        }

        [void]$groupedCommits[$groupKey].Add("  - $description")
    }

    $sortedKeys = Sort-KeysByPreferredOrder -allKeys $groupedCommits.Keys -preferredOrder $script:TypeOrder
    $formattedLines = @()
    foreach ($type in $sortedKeys) {
        if ($groupedCommits[$type].Count -gt 0) {
            $headerName = if ($script:HeaderNameMapping.ContainsKey($type)) { $script:HeaderNameMapping[$type] } else { $type.Substring(0, 1).ToUpper() + $type.Substring(1) }
            $formattedLines += @("- $headerName", "") + @($groupedCommits[$type]) + @("")
        }
    }

    if ($formattedLines.Count -gt 0 -and [string]::IsNullOrWhiteSpace($formattedLines[-1])) {
        if ($formattedLines.Count -gt 1) {
            $formattedLines = $formattedLines[0..($formattedLines.Count - 2)]
        } else {
            $formattedLines = @()
        }
    }

    return $formattedLines
}

# --- SCRIPT FUNCTIONS ---

function Update-CrateVersion {
    param(
        [string]$crateName,
        [string]$version,
        [string]$bump,
        [string]$crateCargoToml,
        [string]$rootCargoToml
    )

    $currentVersion = Get-CurrentVersion -cargoTomlPath $crateCargoToml

    $newVersion = ""
    if ([string]::IsNullOrEmpty($version)) {
        $bumpType = if ([string]::IsNullOrEmpty($bump)) { 'minor' } else { $bump }
        $newVersion = Get-NextVersion -currentVersion $currentVersion -bump $bumpType
        Write-Host "✅ Incrementing $bumpType version from $currentVersion to $newVersion."
    }
    else {
        $newVersion = $version
        Write-Host "✅ Using specified version: $newVersion."
    }

    Write-Host "📝 Updating '$crateCargoToml'..."
    $crateContent = Get-Content $crateCargoToml -Raw
    # Scope the version replacement to the [package] table via the shared regex
    # in releasing.ps1, which anchors to line starts so substring keys like
    # `rust-version` cannot match and inline workspace-dep `version = "..."`
    # declarations later in the file are left alone. Replace exactly once.
    if (-not $script:CargoPackageVersionRegex.IsMatch($crateContent)) {
        Write-Error "Could not find [package] version line in '$crateCargoToml'." -ErrorAction Stop
    }
    $crateContent = $script:CargoPackageVersionRegex.Replace($crateContent, ('${1}' + $newVersion), 1)
    Set-Content $crateCargoToml -Value $crateContent -NoNewline

    Write-Host "📝 Updating '$rootCargoToml'..."

    function Get-EscapedRegexSpecialChars($str) {
        # Escape all regex metacharacters: . $ ^ { [ ( | ) * + ? \ /
        # The replacement string `\$1` produces a literal backslash followed by
        # the matched metacharacter — `\` is a literal in .NET replacement-string
        # syntax (not an escape) and `$1` is the group-1 backreference. Do NOT
        # use `\\$1` here: that double-escapes (e.g. `1.2.3` -> `1\\.2\\.3`).
        return ($str -replace $script:RegexEscapeRegex, '\$1')
    }

    $escapedCrateName = Get-EscapedRegexSpecialChars($crateName)
    $crateNamePattern = $escapedCrateName.Replace('_', '[-_]')
    $regex = '(?<=' + $crateNamePattern + '\s*=\s*\{[^\}]*?version\s*=\s*")[^"]+'
    (Get-Content $rootCargoToml -Raw) -replace $regex, $newVersion | Set-Content $rootCargoToml -NoNewline

    return $newVersion
}

function Write-Changelog {
    param(
        [string]$crateName,
        [string]$newVersion,
        [string]$crateFolder,
        [string]$changelogFile,
        [string]$prBaseUrl,
        # Optional: when this crate is being bumped purely as a cascade from another crate,
        # describe the cascade so a maintenance entry can be written even if the crate has
        # no commits since its last release. Shape: @{ Target = '<name>'; Version = '<x.y.z>'; Breaking = $false }
        [hashtable]$cascadeReason = $null
    )

    $tags = Invoke-Git -Arguments @('tag', '--list', "$crateName-v*")
    $latestTag = $null
    if ($null -eq $tags -or $tags.Count -eq 0) {
        Write-Warning "No tags found for crate '$crateName'. Generating changelog from all history."
    } else {
        $filteredTags = @($tags | Where-Object { $_ -match "^${crateName}-v\d+\.\d+\.\d+$" })
        if ($filteredTags.Count -gt 0) {
            $sortedTags = @($filteredTags | Sort-Object { [version]($_ -replace "${crateName}-v", '') })
            $latestTag = $sortedTags[-1]
        } else {
            Write-Warning "No valid semantic version tags found for crate '$crateName'. Generating changelog from all history."
        }
    }

    $currentDate = (Get-Date).ToString('yyyy-MM-dd')

    # Get commits since the latest tag (unreleased commits)
    $range = if ($latestTag) { "$latestTag..HEAD" } else { "HEAD" }
    $rawCommits = Invoke-Git -Arguments @('log', $range, '--pretty=format:%s', '--', $crateFolder)
    if ($null -eq $rawCommits -or $rawCommits.Count -eq 0) {
        $rawCommits = @()
    } else {
        $rawCommits = @($rawCommits)
    }

    $formattedCommits = @()
    if ($rawCommits.Count -gt 0) {
        $formattedCommits = Format-ConventionalCommits -rawCommitMessages $rawCommits -prBaseUrl $prBaseUrl
    }

    if ($formattedCommits.Count -eq 0 -and $null -eq $cascadeReason) {
        if ($rawCommits.Count -eq 0) {
            Write-Warning "No unreleased commits found to add to the changelog."
        } else {
            Write-Warning "No relevant commits found to add to the changelog (all $($rawCommits.Count) commits were filtered out)."
        }
        return
    }

    # Prepend a cascade entry when this crate is being bumped purely because one of its
    # dependencies was bumped. Emits a structured "Now requires <version> of <target>"
    # bullet (deliberately formal rather than colloquial) under the appropriate section:
    #   - 🔧 Maintenance
    #     - Now requires <version> of `<target>`
    # If the same section header was already produced by Format-ConventionalCommits for this
    # release, the cascade bullet is merged into that existing section instead of creating a
    # duplicate header.
    if ($null -ne $cascadeReason) {
        $sectionHeader = if ($cascadeReason.Breaking) { '- ⚠️ Breaking' } else { '- 🔧 Maintenance' }
        $cascadeBullet = "  - Now requires ``$($cascadeReason.Version)`` of ``$($cascadeReason.Target)``"

        $existingHeaderIdx = -1
        for ($i = 0; $i -lt $formattedCommits.Count; $i++) {
            if ($formattedCommits[$i] -eq $sectionHeader) {
                $existingHeaderIdx = $i
                break
            }
        }

        if ($existingHeaderIdx -ge 0) {
            # Find the end of this section (next top-level "- " header or end of list).
            $insertIdx = $formattedCommits.Count
            for ($i = $existingHeaderIdx + 1; $i -lt $formattedCommits.Count; $i++) {
                if ($formattedCommits[$i] -match '^- \S') { $insertIdx = $i; break }
            }
            # Trim trailing blank lines belonging to the section.
            while ($insertIdx -gt $existingHeaderIdx + 1 -and [string]::IsNullOrWhiteSpace($formattedCommits[$insertIdx - 1])) {
                $insertIdx--
            }
            $before = if ($insertIdx -gt 0) { @($formattedCommits[0..($insertIdx - 1)]) } else { @() }
            $after  = if ($insertIdx -lt $formattedCommits.Count) { @($formattedCommits[$insertIdx..($formattedCommits.Count - 1)]) } else { @() }
            $formattedCommits = $before + @($cascadeBullet) + $after
        } else {
            $cascadeLines = @($sectionHeader, "", $cascadeBullet)
            if ($formattedCommits.Count -gt 0) {
                $formattedCommits = $cascadeLines + @("") + $formattedCommits
            } else {
                $formattedCommits = $cascadeLines
            }
        }
    }

    # Build the new version section
    $newVersionSection = @("## [$newVersion] - $currentDate", "")
    $newVersionSection += $formattedCommits
    $newVersionSection += ""

    # Check if changelog file exists and has content
    if (Test-Path $changelogFile) {
        $existingContent = Get-Content $changelogFile -Raw
        if ($existingContent) {
            # Find the position after "# Changelog" header and any blank lines
            # Insert the new version section there
            $headerPattern = '^# Changelog\s*\r?\n(\r?\n)*'
            if ($existingContent -match $headerPattern) {
                # Match the existing file's line-ending convention so we don't introduce
                # mixed endings (e.g. CRLF body + LF for the new section).
                $eol = Get-FileLineEnding -Path $changelogFile
                $headerMatch = [regex]::Match($existingContent, $headerPattern)
                $insertPosition = $headerMatch.Index + $headerMatch.Length
                $newContent = $existingContent.Substring(0, $insertPosition) +
                              ($newVersionSection -join $eol) + $eol +
                              $existingContent.Substring($insertPosition)
                Set-Content -LiteralPath $changelogFile -Value $newContent -NoNewline -Encoding utf8
                Write-Host "✅ Changelog updated at '$changelogFile'."
                return
            }
        }
    }

    # If no existing changelog or couldn't parse it, create a new one.
    # No existing file to sample from, so default to LF (modern convention; matches
    # what .gitattributes normalizes to in repos that enforce it).
    $changelogContent = @("# Changelog", "")
    $changelogContent += $newVersionSection
    Set-Content -LiteralPath $changelogFile -Value (($changelogContent -join "`n") + "`n") -NoNewline -Encoding utf8
    Write-Host "✅ Changelog created at '$changelogFile'."
}

function Update-Readme {
    param(
        [string]$crateName,
        [string]$crateFolder
    )

    $readmeTemplate = Join-Path $crateFolder "../README.j2"
    if (-not (Test-Path $readmeTemplate)) {
        Write-Warning "README template not found at '$readmeTemplate'. Skipping README generation."
        return
    }

    if (-not (Test-CommandExists -command "cargo-doc2readme")) {
        Write-Warning "cargo-doc2readme is not installed. Skipping README generation. Install with: cargo install cargo-doc2readme"
        return
    }

    Write-Host "📝 Updating README.md..."
    Push-Location $crateFolder
    try {
        $result = cargo doc2readme --lib --template ../README.j2 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "Failed to generate README: $result"
        } else {
            Write-Host "✅ README.md updated."
        }
    }
    finally {
        Pop-Location
    }
}

# Bumps a single crate's version, regenerates its changelog and README.
# Returns the new version string.
function Invoke-CrateRelease {
    param(
        [string]$crateName,
        [string]$crateFolder,
        [string]$crateCargoToml,
        [string]$rootCargoToml,
        [string]$changelogFile,
        [string]$prBaseUrl,
        [string]$version,
        [string]$bump,
        [hashtable]$cascadeReason = $null
    )

    $newVersion = Update-CrateVersion -crateName $crateName -version $version -bump $bump `
        -crateCargoToml $crateCargoToml -rootCargoToml $rootCargoToml
    if ($null -eq $newVersion) {
        Write-Error "Failed to update version for crate '$crateName'." -ErrorAction Stop
    }

    Write-Changelog -crateName $crateName -newVersion $newVersion -crateFolder $crateFolder `
        -changelogFile $changelogFile -prBaseUrl $prBaseUrl -cascadeReason $cascadeReason
    Update-Readme -crateName $crateName -crateFolder $crateFolder

    return $newVersion
}

function Show-ReleaseSummary {
    param(
        [array]$releases
    )

    Write-Host ""
    Write-Host "📦 Released crates:" -ForegroundColor Green
    foreach ($r in $releases) {
        Write-Host "  - $($r.Crate): $($r.OldVersion) -> $($r.NewVersion)" -ForegroundColor Green
    }
    Write-Host ""
}

function Show-FinalMessage {
    param(
        [Parameter(Mandatory = $true)][string]$CrateName,
        [Parameter(Mandatory = $true)][array]$Releases
    )

    # Locate the primary release record (the package the user originally asked
    # for). Defensive fallback to the first release in case it's missing — this
    # shouldn't happen in practice but we never want the post-success message
    # to crash and stamp the run as failed.
    $primary = $Releases | Where-Object { $_.Crate -eq $CrateName } | Select-Object -First 1
    if ($null -eq $primary) { $primary = $Releases | Select-Object -First 1 }
    $primaryName    = $primary.Crate
    $primaryVersion = $primary.NewVersion

    $extraCount = @($Releases).Count - 1
    if ($extraCount -le 0) {
        # Single-package release: a scoped feat(<crate>): prefix is the most
        # informative form because the commit really is about that one crate.
        $commitMessage = "feat($primaryName): release v$primaryVersion"
    } else {
        # Multi-package release: the conventional-commits scope would be
        # misleading because the commit spans many packages. Drop the scope
        # and call out the extras so reviewers see at a glance that this is
        # a coordinated release, not a single-crate bump.
        $extraNoun = if ($extraCount -eq 1) { 'additional package' } else { 'additional packages' }
        $commitMessage = "feat: release $primaryName v$primaryVersion and $extraCount $extraNoun"
    }

    Write-Host "---" -ForegroundColor Green
    Write-Host "🎉 Success! Next steps:" -ForegroundColor Green
    Write-Host "1. Review the changes in the updated files." -ForegroundColor Green
    Write-Host "2. Commit the changes and push the changes:" -ForegroundColor Green
    Write-Host "   git add ." -ForegroundColor DarkGray
    Write-Host "   git commit -m `"$commitMessage`"" -ForegroundColor DarkGray
    # Plain `git push` is sufficient because we just committed to the current
    # branch; no need to substitute a placeholder branch name into the snippet.
    Write-Host "   git push" -ForegroundColor DarkGray
    Write-Host "3. Once the commit is merged to main, automation will tag the commit and release to crates.io" -ForegroundColor Green
    Write-Host "---" -ForegroundColor Green
}

# --- POST-RELEASE SCAN HELPERS ---

# Idempotently inserts a "Now requires <version> of <target>" bullet into an
# existing `## [<Version>]` section in a changelog. Used when a dependent has
# already been version-bumped (sufficiently) in an earlier cascade pass within the
# same PR — we don't want to re-bump, but we still want to record that this new
# release also pulled through. Operates by reading the file, locating the target
# section, finding (or creating) the appropriate `- 🔧 Maintenance` or `- ⚠️ Breaking`
# sub-header, and inserting the bullet unless an exact match already exists.
function Add-CascadeBulletToVersionSection {
    param(
        [Parameter(Mandatory = $true)][string]$ChangelogFile,
        [Parameter(Mandatory = $true)][string]$Version,
        [Parameter(Mandatory = $true)][hashtable]$CascadeReason
    )

    if (-not (Test-Path $ChangelogFile)) {
        Write-Warning "Add-CascadeBulletToVersionSection: changelog '$ChangelogFile' does not exist; skipping."
        return
    }

    $targetName    = $CascadeReason.Target
    $targetVersion = $CascadeReason.Version
    $isBreaking    = [bool]$CascadeReason.Breaking
    $subHeader     = if ($isBreaking) { '- ⚠️ Breaking' } else { '- 🔧 Maintenance' }
    $bullet        = "  - Now requires ``$targetVersion`` of ``$targetName``"

    $lines = @(Get-Content -LiteralPath $ChangelogFile)
    $escapedVersion = $script:RegexEscapeRegex.Replace($Version, '\$1')
    $sectionStart = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match "^## \[$escapedVersion\]") { $sectionStart = $i; break }
    }
    if ($sectionStart -lt 0) {
        Write-Warning "Add-CascadeBulletToVersionSection: no `## [$Version]` section in '$ChangelogFile'; skipping."
        return
    }

    $sectionEnd = $lines.Count
    for ($i = $sectionStart + 1; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^## \[') { $sectionEnd = $i; break }
    }

    $subStart = -1
    for ($i = $sectionStart + 1; $i -lt $sectionEnd; $i++) {
        if ($lines[$i] -eq $subHeader) { $subStart = $i; break }
    }

    if ($subStart -ge 0) {
        $subEnd = $sectionEnd
        for ($i = $subStart + 1; $i -lt $sectionEnd; $i++) {
            if ($lines[$i] -match '^- ') { $subEnd = $i; break }
        }
        for ($i = $subStart + 1; $i -lt $subEnd; $i++) {
            if ($lines[$i] -eq $bullet) { return } # already present
        }
        $insertAt = $subEnd
        # Walk backwards past trailing blank lines so the bullet stays adjacent to the sub-section.
        while ($insertAt -gt $subStart + 1 -and [string]::IsNullOrWhiteSpace($lines[$insertAt - 1])) {
            $insertAt--
        }
        if ($insertAt -eq $lines.Count) {
            # Inserting at EOF: avoid the reverse-range slice `$lines[$lines.Count..($lines.Count - 1)]`,
            # which silently aliases to the last element and duplicates it.
            $new = @($lines[0..($insertAt - 1)]) + @($bullet)
        } else {
            $new = @($lines[0..($insertAt - 1)]) + @($bullet) + @($lines[$insertAt..($lines.Count - 1)])
        }
    }
    else {
        $insertAt = $sectionEnd
        while ($insertAt -gt $sectionStart + 1 -and [string]::IsNullOrWhiteSpace($lines[$insertAt - 1])) {
            $insertAt--
        }
        $block = @('', $subHeader, $bullet)
        if ($insertAt -eq $lines.Count) {
            $new = @($lines[0..($insertAt - 1)]) + $block
        } else {
            $new = @($lines[0..($insertAt - 1)]) + $block + @($lines[$insertAt..($lines.Count - 1)])
        }
    }

    # Match the existing file's line-ending convention rather than hardcoding LF —
    # a string-array passed to Set-Content joins with [Environment]::NewLine (CRLF on
    # Windows), which produces noisy whole-file diffs in LF-normalized repos and
    # mixed endings in repos that genuinely use CRLF.
    $eol = Get-FileLineEnding -Path $ChangelogFile
    $hadTrailingNewline = ($lines.Count -gt 0) -and ($lines[-1] -eq '')
    $body = ($new -join $eol)
    if ($hadTrailingNewline -and -not $body.EndsWith($eol)) { $body += $eol }
    Set-Content -LiteralPath $ChangelogFile -Value $body -NoNewline -Encoding utf8
    Write-Host "📝 Recorded cascade in '$ChangelogFile' under [$Version]." -ForegroundColor DarkCyan
}

# Cascades a single dependent. Re-bump-safe: if the dependent has already been bumped
# during this PR (its on-disk version differs from $BaseRef) we either skip the bump
# (when the existing bump is sufficient) or upgrade to the required version.
function Invoke-CascadeStep {
    param(
        [Parameter(Mandatory = $true)][string]$Dependent,
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$PrBaseUrl,
        [Parameter(Mandatory = $true)][string]$TargetCrateName,
        [Parameter(Mandatory = $true)][string]$TargetNewVersion,
        [Parameter(Mandatory = $true)][string]$DepBump,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$BaseRef
    )

    $depFolder = Join-Path $RepoRoot 'crates' $Dependent
    $depCargo  = Join-Path $depFolder 'Cargo.toml'
    $depChange = Join-Path $depFolder 'CHANGELOG.md'

    if (-not (Test-Path $depCargo)) {
        Write-Warning "Skipping cascade for '$Dependent': Cargo.toml not found at '$depCargo'."
        return $null
    }

    $depCurrent = Get-CurrentVersion -cargoTomlPath $depCargo
    $depBase = if (-not [string]::IsNullOrEmpty($BaseRef)) {
        Get-CrateVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -CrateFolder $Dependent
    } else { $null }

    $depCascadeReason = @{
        Target   = $TargetCrateName
        Version  = $TargetNewVersion
        Breaking = (Test-IsBreakingChange -oldVersion $depCurrent -bump $DepBump)
    }

    # New crate (no base-ref Cargo.toml) or no usable base ref: behave as the legacy
    # cascade did — let Invoke-CrateRelease bump from $depCurrent.
    if ([string]::IsNullOrEmpty($depBase) -or $depCurrent -eq $depBase) {
        $depNew = Invoke-CrateRelease -crateName $Dependent -crateFolder $depFolder `
            -crateCargoToml $depCargo -rootCargoToml $RootCargoToml -changelogFile $depChange `
            -prBaseUrl $PrBaseUrl -version "" -bump $DepBump -cascadeReason $depCascadeReason
        Invalidate-WorkspaceMetadataCache
        return [pscustomobject]@{ Crate = $Dependent; OldVersion = $depCurrent; NewVersion = $depNew }
    }

    # Already bumped in this PR. Compute the version that THIS cascade would have
    # produced starting from the base-ref version, and compare.
    $required = Get-NextVersion -currentVersion $depBase -bump $DepBump
    $cmp = Compare-SemanticVersions -version1 $depCurrent -version2 $required

    if ($cmp -ge 0) {
        Write-Host "  • $Dependent already at $depCurrent (>= required $required); recording cascade only." -ForegroundColor DarkGray
        Add-CascadeBulletToVersionSection -ChangelogFile $depChange -Version $depCurrent -CascadeReason $depCascadeReason
        return [pscustomobject]@{ Crate = $Dependent; OldVersion = $depCurrent; NewVersion = $depCurrent }
    }

    Write-Host "  • $Dependent currently $depCurrent < required $required; upgrading." -ForegroundColor DarkYellow
    $depNew = Invoke-CrateRelease -crateName $Dependent -crateFolder $depFolder `
        -crateCargoToml $depCargo -rootCargoToml $RootCargoToml -changelogFile $depChange `
        -prBaseUrl $PrBaseUrl -version $required -bump "" -cascadeReason $depCascadeReason
    Invalidate-WorkspaceMetadataCache
    return [pscustomobject]@{ Crate = $Dependent; OldVersion = $depCurrent; NewVersion = $depNew }
}

# --- CASCADE-MESSAGE FORMATTING ---
#
# Pure helpers backing the cascade announcement printed by Invoke-ReleaseFlow.
# Split out so the human-facing wording (and the "downgrade by one level"
# mapping for non-exposing dependents) can be unit-tested without driving the
# full release flow.

# Maps an internal bump kind to the semantic label shown in the cascade
# announcement (full form, used as 'as <label>'): major → 'breaking change',
# minor → 'non-breaking change', patch → 'patch'.
function Get-ChangeLabelFromBumpKind {
    param([Parameter(Mandatory = $true)][ValidateSet('major', 'minor', 'patch')][string]$BumpKind)

    switch ($BumpKind) {
        'major' { return 'breaking change' }
        'minor' { return 'non-breaking change' }
        'patch' { return 'patch' }
    }
}

# Short form of the semantic label, used inside the parenthetical that
# describes the downgrade for non-exposing dependents (e.g. "or non-breaking
# if no API exposure of '<target>'"). Mirrors Get-ChangeLabelFromBumpKind
# without the trailing 'change' noun where it would read awkwardly.
function Get-ShortChangeLabelFromBumpKind {
    param([Parameter(Mandatory = $true)][ValidateSet('major', 'minor', 'patch')][string]$BumpKind)

    switch ($BumpKind) {
        'major' { return 'breaking' }
        'minor' { return 'non-breaking' }
        'patch' { return 'patch' }
    }
}

# Builds the cascade announcement line. ExposingBump is what we apply to
# dependents that re-export the target's types in their public API;
# NonExposingBump is what we apply to internal-only consumers (today: always
# 'patch'). When the two are identical (i.e. the target itself is being
# released as a patch, so the non-exposing bump cannot go any lower), the
# parenthetical clause is suppressed entirely — saying "or patch if no API
# exposure" would just repeat the headline label.
function Format-CascadeAnnouncement {
    param(
        [Parameter(Mandatory = $true)][ValidateSet('major', 'minor', 'patch')][string]$ExposingBump,
        [Parameter(Mandatory = $true)][ValidateSet('major', 'minor', 'patch')][string]$NonExposingBump,
        [Parameter(Mandatory = $true)][string]$TargetCrateName,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$DependentNames
    )

    $count = @($DependentNames).Count
    $noun  = if ($count -eq 1) { 'dependent package' } else { 'dependent packages' }

    $headlineLabel = Get-ChangeLabelFromBumpKind -BumpKind $ExposingBump

    if ($ExposingBump -eq $NonExposingBump) {
        $parenthetical = ''
    } else {
        $downgradeLabel = Get-ShortChangeLabelFromBumpKind -BumpKind $NonExposingBump
        $parenthetical = " (or $downgradeLabel if no API exposure of ``$TargetCrateName``)"
    }

    return "🔗 Cascading release to $count $noun as $headlineLabel$parenthetical`: $($DependentNames -join ', ')"
}

# Per-dependent "  • <name> -> <semantic-bump-label> (<why>)" line printed
# under the cascade announcement. The bump label is the SHORT semantic form
# (breaking / non-breaking / patch) so the readout matches the announcement's
# vocabulary instead of leaking the internal Cargo bump kind. The why-clause
# tells the user what drove the bump choice — public-API exposure vs. internal
# use — so the reader can quickly sanity-check whether the inferred exposure
# matches their mental model.
function Format-CascadeDependentLine {
    param(
        [Parameter(Mandatory = $true)][string]$DependentName,
        [Parameter(Mandatory = $true)][ValidateSet('major', 'minor', 'patch')][string]$BumpKind,
        [Parameter(Mandatory = $true)][bool]$ExposesTarget
    )

    $label   = Get-ShortChangeLabelFromBumpKind -BumpKind $BumpKind
    $why     = if ($ExposesTarget) { 'exposes target in public API' } else { 'internal use only' }
    return "  • $DependentName -> $label ($why)"
}

# Pure formatter for the "Detected pending uncommitted releases ..." block printed
# at the top of Invoke-ReleaseMain. Each pending record is a [pscustomobject] with
# Name, BaseVersion, CurrentVersion (Get-PendingReleases produces these in stable
# Folder order). Returns '' when there are no pending releases so the caller can
# unconditionally print and rely on Write-Host to no-op on empty input.
#
# Format:
#   Detected pending uncommitted releases and included in analysis data set:
#      <name1> <base1> -> <current1>
#      <name2> <base2> -> <current2>
function Format-PendingReleasesAnnouncement {
    param(
        [Parameter(Mandatory = $true)][AllowNull()][AllowEmptyCollection()]$Pending
    )

    if ($null -eq $Pending) { return '' }
    $items = @($Pending)
    if ($items.Count -eq 0) { return '' }

    $lines = @('Detected pending uncommitted releases and included in analysis data set:')
    foreach ($entry in $items) {
        $lines += "   $($entry.Name) $($entry.BaseVersion) -> $($entry.CurrentVersion)"
    }
    return ($lines -join [Environment]::NewLine)
}

# Runs the bump + downstream cascade for a single target crate. Returns the augmented
# $releases array. Equivalent to the legacy inline body, but factored so the post-release
# scan can invoke it recursively for upstream dependencies the user agrees to release.
function Invoke-ReleaseFlow {
    param(
        [Parameter(Mandatory = $true)][string]$CrateName,
        [Parameter(Mandatory = $false)][string]$Version = '',
        [Parameter(Mandatory = $false)][string]$Bump = '',
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $false)][string]$PrBaseUrl,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$BaseRef
    )

    $crateFolder    = Join-Path $RepoRoot 'crates' $CrateName
    $crateCargoToml = Join-Path $crateFolder 'Cargo.toml'
    $changelogFile  = Join-Path $crateFolder 'CHANGELOG.md'

    $currentVersion = Get-CurrentVersion -cargoTomlPath $crateCargoToml
    if ([string]::IsNullOrWhiteSpace($currentVersion)) {
        Write-Error "Failed to determine current version for '$CrateName'. Aborting."
        Exit 1
    }

    $baseVersion = if (-not [string]::IsNullOrEmpty($BaseRef)) {
        Get-CrateVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -CrateFolder $CrateName
    } else { $null }

    # Re-invocation on a primary target that already has a pending uncommitted
    # version bump (e.g. an earlier `release-crate.ps1` invocation in the same PR).
    # Mirrors the base-relative no-op/upgrade logic Invoke-CascadeStep already
    # applies to dependents: compute the version this invocation WOULD have
    # produced starting from the base-ref version, and compare with the on-disk
    # current version.
    $isPendingPrimary = (-not [string]::IsNullOrEmpty($baseVersion)) -and ($currentVersion -ne $baseVersion)

    if ($isPendingPrimary) {
        $requiredVersion = if (-not [string]::IsNullOrEmpty($Version)) {
            $Version
        } elseif (-not [string]::IsNullOrEmpty($Bump)) {
            Get-NextVersion -currentVersion $baseVersion -bump $Bump
        } else {
            # Default-bump (no -Version, no -Bump) matches Invoke-CrateRelease's
            # internal default of 'minor'. Keeps re-invocation idempotent with
            # the initial bare-call.
            Get-NextVersion -currentVersion $baseVersion -bump 'minor'
        }

        $cmp = Compare-SemanticVersions -version1 $currentVersion -version2 $requiredVersion

        if ($cmp -gt 0 -and -not [string]::IsNullOrEmpty($Version)) {
            # Explicit -Version asks for something lower than the pending current.
            # Treat as a likely user mistake (typo, stale flag) rather than silently
            # no-opping into the higher pending version.
            Write-Error "Cannot release '$CrateName' as v${Version}: package is already pending at v$currentVersion (base v$baseVersion). Explicit -Version downgrades are not supported."
            Exit 1
        }

        if ($cmp -ge 0) {
            # No-op for the primary. The on-disk Cargo.toml + changelog from the
            # prior invocation already reflect the intended release. Cascade still
            # runs because dependents may benefit from another idempotent pass.
            Write-Host "ℹ️  '$CrateName' already pending at v$currentVersion (base v$baseVersion); skipping primary bump." -ForegroundColor DarkGray
            $oldVersion = $baseVersion
            $newVersion = $currentVersion

            # Cascade bump derives from the EFFECTIVE base→current transition,
            # not the user-requested bump. Otherwise a re-invocation with a
            # weaker -Change (e.g. Patch on a previously-minor-bumped primary)
            # would under-cascade dependents that need the stronger bump to
            # stay compatible with the on-disk API changes.
            $cascadeBump = Get-BumpKindFromVersions -oldVersion $baseVersion -newVersion $currentVersion
        } else {
            # cmp < 0: requested release would escalate the primary above its
            # current pending version. We don't support automated escalation
            # (the existing changelog section would need to be merged into the
            # new one, which is non-trivial); ask the user to restore the
            # pending artifacts and re-invoke from a clean state.
            $artifactsHint = "crates/$CrateName/Cargo.toml, crates/$CrateName/CHANGELOG.md, crates/$CrateName/README.md, and the workspace Cargo.toml entry for '$CrateName'"
            Write-Error "Cannot escalate pending release of '$CrateName': already pending at v$currentVersion (base v$baseVersion), but the requested change requires at least v$requiredVersion. To re-do the release at a higher version, first restore the previous pending release artifacts ($artifactsHint) to their base-ref state, then re-invoke."
            Exit 1
        }
    } else {
        $oldVersion = $currentVersion
        $newVersion = Invoke-CrateRelease -crateName $CrateName -crateFolder $crateFolder `
            -crateCargoToml $crateCargoToml -rootCargoToml $RootCargoToml -changelogFile $changelogFile `
            -prBaseUrl $PrBaseUrl -version $Version -bump $Bump
        Invalidate-WorkspaceMetadataCache

        $cascadeBump = if (-not [string]::IsNullOrEmpty($Bump)) {
            $Bump
        } elseif (-not [string]::IsNullOrEmpty($Version)) {
            Get-BumpKindFromVersions -oldVersion $oldVersion -newVersion $newVersion
        } else {
            'minor'
        }
    }

    $releases = @(
        [pscustomobject]@{ Crate = $CrateName; OldVersion = $oldVersion; NewVersion = $newVersion }
    )

    $targetIsBreaking = Test-IsBreakingChange -oldVersion $oldVersion -bump $cascadeBump
    $exposingCascadeBump = if ($targetIsBreaking) { 'major' } else { $cascadeBump }

    $dependents = @(Get-AllTransitiveDependents -crateName $CrateName -repoRoot $RepoRoot)
    if ($dependents.Count -gt 0) {
        Write-Host ""
        $cascadeMessage = Format-CascadeAnnouncement -ExposingBump $exposingCascadeBump `
            -NonExposingBump 'patch' -TargetCrateName $CrateName -DependentNames $dependents
        Write-Host $cascadeMessage -ForegroundColor Cyan

        $allCrates = Get-WorkspaceCrates -repoRoot $RepoRoot
        $targetCrate = $allCrates | Where-Object { $_.Folder -eq $CrateName -or $_.Name -eq $CrateName } | Select-Object -First 1
        $targetPackageName = if ($null -ne $targetCrate) { $targetCrate.Name } else { $CrateName }

        foreach ($dependent in $dependents) {
            $depCrate = $allCrates | Where-Object { $_.Folder -eq $dependent } | Select-Object -First 1
            $exposes = if ($null -ne $depCrate) {
                Test-CrateExposesTarget -dependent $depCrate -targetPackageName $targetPackageName
            } else { $true }

            $depBump = if ($exposes) { $exposingCascadeBump } else { 'patch' }
            Write-Host (Format-CascadeDependentLine -DependentName $dependent -BumpKind $depBump -ExposesTarget $exposes) -ForegroundColor DarkCyan

            $record = Invoke-CascadeStep -Dependent $dependent -RepoRoot $RepoRoot `
                -RootCargoToml $RootCargoToml -PrBaseUrl $PrBaseUrl `
                -TargetCrateName $CrateName -TargetNewVersion $newVersion `
                -DepBump $depBump -BaseRef $BaseRef
            if ($null -ne $record) {
                $releases += $record
            }
        }
    }

    return $releases
}

function Test-InteractiveSession {
    if ($env:CI) { return $false }
    if ($env:GITHUB_ACTIONS) { return $false }
    try { if ([Console]::IsInputRedirected) { return $false } } catch { }
    return $true
}

# --- POST-RELEASE-SCAN PROMPT FLOW ---
#
# Helpers backing Invoke-PostReleaseDepScan's per-package menu. Split out so
# pure formatting can be unit-tested without capturing host streams, and so the
# diff / opener side-effects can be mocked individually.

# Tracks temp files produced by Show-PackageDiff so Invoke-PostReleaseDepScan
# can delete them at the end of the run. The scan entrypoint save/restores
# this so nested or re-entrant invocations don't clobber an outer run's list.
$script:TempPackageDiffPaths = [System.Collections.Generic.List[string]]::new()

# Returns $true when option 5 (Release as patch) would be numerically indistinguishable
# from option 4 (Release as non-breaking change) for the given current version.
# This is the case for Cargo 0.x.y versions, where the semver carve-out lumps
# minor and patch under the same numeric increment (0.x.(y+1)) — and on 0.0.x
# where every bump collapses to 0.0.(x+1). When CurrentVersion is unknown, we
# conservatively return $false so all options remain visible.
function Test-IsPatchOptionRedundant {
    param([Parameter(Mandatory = $true)][AllowNull()][AllowEmptyString()][string]$CurrentVersion)

    if ([string]::IsNullOrWhiteSpace($CurrentVersion)) { return $false }
    $minorNext = Get-NextVersion -currentVersion $CurrentVersion -bump 'minor'
    $patchNext = Get-NextVersion -currentVersion $CurrentVersion -bump 'patch'
    return ($minorNext -eq $patchNext)
}

# Pure formatter for the per-package menu. Returns a multi-line string ready
# for Write-Host. Returning a string (not host-writing directly) keeps the
# function unit-testable without redirecting Information / Host streams.
#
# Options 3-5 render the *concrete* version transition each choice would
# produce (e.g. "Release as breaking change (0.1.2 -> 0.2.0)"). Get-NextVersion
# is the single source of truth for the major/minor/patch math and already
# honours Cargo's 0.x.y semver carve-outs, so the menu always shows the same
# version the release would produce — not a misleading numeric label.
#
# Option 5 (Release as patch) is hidden when it would produce the same numeric
# bump as option 4 (Release as non-breaking change) — see
# Test-IsPatchOptionRedundant. This avoids presenting two indistinguishable
# choices on Cargo 0.x.y packages.
function Format-PackageMenu {
    param(
        [Parameter(Mandatory = $true)][object]$Finding,
        [Parameter(Mandatory = $true)][int]$RemainingCount
    )

    $folder = [string]$Finding.Folder
    if ($RemainingCount -gt 0) {
        $word = if ($RemainingCount -eq 1) { 'package' } else { 'packages' }
        $queueSuffix = " (+$RemainingCount $word queued)"
    } else {
        $queueSuffix = ''
    }

    # Build the version-transition annotations for options 3-5. CurrentVersion
    # may be missing on hand-crafted test findings or in unusual non-cargo
    # contexts — in that case omit the annotation rather than crash, so the
    # menu still presents the choice (the release flow itself will fail loudly
    # later if there's truly no version).
    $current = [string]$Finding.CurrentVersion
    $bumpHints = @{}
    foreach ($kind in @('major', 'minor', 'patch')) {
        if ([string]::IsNullOrWhiteSpace($current)) {
            $bumpHints[$kind] = "($kind version)"
        } else {
            $next = Get-NextVersion -currentVersion $current -bump $kind
            $bumpHints[$kind] = "($current -> $next)"
        }
    }

    $hidePatch = Test-IsPatchOptionRedundant -CurrentVersion $current

    $sb = [System.Text.StringBuilder]::new()
    [void]$sb.AppendLine('')
    [void]$sb.AppendLine("Detected package with unreleased modifications: $folder$queueSuffix")
    [void]$sb.AppendLine('  potentially affected dependency chains:')
    foreach ($chain in @($Finding.DependencyChains)) {
        [void]$sb.AppendLine("    $($chain -join ' -> ')")
    }
    [void]$sb.AppendLine('')
    [void]$sb.AppendLine('  1. View diff')
    [void]$sb.AppendLine('  2. Ignore package - the changes are immaterial to published functionality')
    [void]$sb.AppendLine("  3. Release as breaking change $($bumpHints['major'])")
    [void]$sb.AppendLine("  4. Release as non-breaking change $($bumpHints['minor'])")
    if (-not $hidePatch) {
        [void]$sb.AppendLine("  5. Release as patch $($bumpHints['patch'])")
    }
    return $sb.ToString()
}

# Writes the menu via Write-Host. Side-effect wrapper around Format-PackageMenu
# so the pure formatter stays test-friendly.
function Show-PackageMenu {
    param(
        [Parameter(Mandatory = $true)][object]$Finding,
        [Parameter(Mandatory = $true)][int]$RemainingCount
    )
    Write-Host (Format-PackageMenu -Finding $Finding -RemainingCount $RemainingCount)
}

# Builds the diff text for a single package, anchored at its last release
# baseline (Get-CrateLastReleaseBaseline). When no baseline is found (e.g.
# a never-released crate), falls back to `git diff HEAD` and prefixes the
# diff with a warning header so the reader knows the anchor is not a true
# prior release. Untracked files are appended as plain content blocks
# (git diff itself does not include untracked content).
function Get-PackageDiffText {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$Folder
    )

    $sb = [System.Text.StringBuilder]::new()
    $relRoot = "crates/$Folder"

    $baseline = Get-CrateLastReleaseBaseline -RepoRoot $RepoRoot -CrateFolder $Folder
    if ([string]::IsNullOrWhiteSpace($baseline)) {
        [void]$sb.AppendLine("# Diff of '$Folder' (no prior version/publish baseline found - showing working tree vs HEAD)")
        [void]$sb.AppendLine('')
        $diff = Invoke-Git -Arguments @('diff', 'HEAD', '--', $relRoot) -RepoRoot $RepoRoot -AllowFailure
    } else {
        [void]$sb.AppendLine("# Diff of '$Folder' since $baseline")
        [void]$sb.AppendLine('')
        $diff = Invoke-Git -Arguments @('diff', $baseline, '--', $relRoot) -RepoRoot $RepoRoot -AllowFailure
    }

    if ($null -ne $diff) {
        foreach ($line in @($diff)) {
            [void]$sb.AppendLine($line.ToString())
        }
    }

    $untracked = Invoke-Git -Arguments @('ls-files', '--others', '--exclude-standard', '--', $relRoot) -RepoRoot $RepoRoot -AllowFailure
    if ($null -ne $untracked) {
        foreach ($line in @($untracked)) {
            $relPath = $line.ToString().Trim().Replace('\', '/')
            if ([string]::IsNullOrEmpty($relPath)) { continue }
            $absPath = Join-Path $RepoRoot $relPath
            [void]$sb.AppendLine('')
            [void]$sb.AppendLine("===== UNTRACKED FILE: $relPath =====")
            if (Test-Path -LiteralPath $absPath) {
                try {
                    $content = Get-Content -LiteralPath $absPath -Raw -ErrorAction Stop
                    if ($null -ne $content) { [void]$sb.Append($content) }
                    if ($null -eq $content -or $content.Length -eq 0 -or -not $content.EndsWith("`n")) {
                        [void]$sb.AppendLine('')
                    }
                } catch {
                    [void]$sb.AppendLine("<could not read file: $_>")
                }
            } else {
                [void]$sb.AppendLine('<file no longer present on disk>')
            }
            [void]$sb.AppendLine('===== END UNTRACKED FILE =====')
        }
    }

    return $sb.ToString()
}

# Writes the given diff text to a uniquely-named file under the OS temp
# directory (or -Directory, for tests) and returns the resulting path. The
# extension defaults to .txt for safe handling by arbitrary text editors;
# pass -Extension '.diff' when the file will be opened in an editor that
# recognises the diff syntax by extension (e.g. VS Code).
function Save-PackageDiffToTempFile {
    param(
        [Parameter(Mandatory = $true)][string]$Folder,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$DiffText,
        [string]$Directory,
        [string]$Extension = '.txt'
    )

    if (-not $Directory) { $Directory = [System.IO.Path]::GetTempPath() }
    if (-not (Test-Path -LiteralPath $Directory)) {
        New-Item -ItemType Directory -Path $Directory -Force | Out-Null
    }
    if (-not $Extension.StartsWith('.')) { $Extension = '.' + $Extension }

    $safeFolder = ($Folder -replace '[^A-Za-z0-9._-]', '_')
    $fileName = "oxi-pkg-diff-$safeFolder-$([guid]::NewGuid().ToString('N'))$Extension"
    $fullPath = Join-Path $Directory $fileName
    Set-Content -LiteralPath $fullPath -Value $DiffText -NoNewline
    return $fullPath
}

# Picks the editor used to render the package diff. Prefers VS Code
# (`code`, then `code-insiders`) because VS Code provides diff syntax
# highlighting out of the box for `.diff` files. Falls back to whatever
# the OS associates with the chosen file extension (handled by
# Open-PathWithPreferredEditor) and to `.txt` so plain text editors can
# always open the file without a "no application registered" error.
#
# Returns @{ Kind = 'code' | 'code-insiders' | 'system'; FileExtension = '.diff' | '.txt' }
function Get-PreferredEditor {
    foreach ($cmd in @('code', 'code-insiders')) {
        if (Get-Command $cmd -ErrorAction SilentlyContinue) {
            return [pscustomobject]@{
                Kind          = $cmd
                FileExtension = '.diff'
            }
        }
    }
    return [pscustomobject]@{
        Kind          = 'system'
        FileExtension = '.txt'
    }
}

# Opens a path with the preferred editor (see Get-PreferredEditor). When
# `-Editor` is omitted the preferred editor is resolved on the fly.
# Non-blocking; never throws — a failure (no VS Code, no association,
# missing system opener) degrades to a Write-Warning so the calling
# release flow continues.
#
# Platform-aware system-default dispatch is needed because PowerShell
# Core's Start-Process expects an executable on non-Windows platforms,
# not a document path.
function Open-PathWithPreferredEditor {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $false)][object]$Editor
    )

    if ($null -eq $Editor) { $Editor = Get-PreferredEditor }

    try {
        if ($Editor.Kind -eq 'code') {
            & code $Path
            if ($LASTEXITCODE -ne 0) { throw "code exited with code $LASTEXITCODE" }
            return
        }
        if ($Editor.Kind -eq 'code-insiders') {
            & code-insiders $Path
            if ($LASTEXITCODE -ne 0) { throw "code-insiders exited with code $LASTEXITCODE" }
            return
        }

        # System default dispatch.
        $onWindows = $false
        $platformVar = Get-Variable -Name IsWindows -Scope Global -ErrorAction SilentlyContinue
        if ($null -eq $platformVar) {
            $onWindows = $true
        } else {
            $onWindows = [bool]$platformVar.Value
        }

        if ($onWindows) {
            Start-Process -FilePath $Path -ErrorAction Stop | Out-Null
            return
        }

        if ($IsMacOS) {
            & open $Path
            if ($LASTEXITCODE -ne 0) { throw "open exited with code $LASTEXITCODE" }
            return
        }

        $xdg = Get-Command xdg-open -ErrorAction SilentlyContinue
        if ($xdg) {
            & xdg-open $Path
            if ($LASTEXITCODE -ne 0) { throw "xdg-open exited with code $LASTEXITCODE" }
            return
        }

        $gio = Get-Command gio -ErrorAction SilentlyContinue
        if ($gio) {
            & gio open $Path
            if ($LASTEXITCODE -ne 0) { throw "gio open exited with code $LASTEXITCODE" }
            return
        }

        throw 'No system file-opener found (tried xdg-open, gio).'
    } catch {
        Write-Warning "Could not open '$Path' with the preferred editor ($($Editor.Kind)): $_"
    }
}

# Renders the package's diff to a temp file, prints the path, and tries to
# open it with the preferred editor (VS Code if available, otherwise the
# OS default opener). The temp file is tracked in
# $script:TempPackageDiffPaths so Invoke-PostReleaseDepScan can clean up.
function Show-PackageDiff {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$Folder
    )

    $diffText = Get-PackageDiffText -RepoRoot $RepoRoot -Folder $Folder
    $editor   = Get-PreferredEditor
    $tempPath = Save-PackageDiffToTempFile -Folder $Folder -DiffText $diffText -Extension $editor.FileExtension

    if ($null -eq $script:TempPackageDiffPaths) {
        $script:TempPackageDiffPaths = [System.Collections.Generic.List[string]]::new()
    }
    [void]$script:TempPackageDiffPaths.Add($tempPath)

    Write-Host ''
    Write-Host "Diff written to: $tempPath" -ForegroundColor Cyan
    Open-PathWithPreferredEditor -Path $tempPath -Editor $editor
}

# Renders the menu for a single finding and runs the input-validation loop.
# Choice 1 (View diff) shows the diff and re-prompts WITHOUT re-rendering
# the menu (the options are still visible higher in the scrollback); choices
# 2..N resolve to a release action. Empty input silently re-prompts (no
# warning), anything else complains then re-prompts. Returns @{ Action =
# 'ignore' | 'major' | 'minor' | 'patch' }.
#
# When option 5 is suppressed (because it would be numerically identical to
# option 4 — see Test-IsPatchOptionRedundant), the prompt range tightens to
# [1-4] and "5" is treated as an invalid choice. This keeps the prompt
# honest with what the menu shows.
function Get-PackageReleaseDecision {
    param(
        [Parameter(Mandatory = $true)][object]$Finding,
        [Parameter(Mandatory = $true)][int]$RemainingCount,
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    $hidePatch = Test-IsPatchOptionRedundant -CurrentVersion ([string]$Finding.CurrentVersion)
    $maxChoice = if ($hidePatch) { 4 } else { 5 }

    Show-PackageMenu -Finding $Finding -RemainingCount $RemainingCount
    while ($true) {
        $raw = Read-Host "Choose option for '$($Finding.Folder)' [1-$maxChoice]"
        $choice = if ($null -eq $raw) { '' } else { $raw.Trim() }

        if ($choice -eq '') { continue }
        if ($choice -eq '1') {
            Show-PackageDiff -RepoRoot $RepoRoot -Folder $Finding.Folder
            continue
        }
        if ($choice -eq '2') { return @{ Action = 'ignore' } }
        if ($choice -eq '3') { return @{ Action = 'major' } }
        if ($choice -eq '4') { return @{ Action = 'minor' } }
        if ($choice -eq '5' -and -not $hidePatch) { return @{ Action = 'patch' } }

        Write-Host "Invalid choice '$choice'. Enter a number from 1 to $maxChoice." -ForegroundColor Yellow
    }
}

# Scans for workspace crates with unreleased modifications (changes newer than the
# crate's own last `version =` / `publish =` commit) that are transitively pulled in
# by a release-set member but are not themselves part of the release set, prompting
# the user (when interactive) to optionally release them too.
# Newly-released crates are appended to the release records via [ref].
function Invoke-PostReleaseDepScan {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$BaseRef,
        [Parameter(Mandatory = $true)][ref]$ReleasesRef,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $false)][string]$PrBaseUrl,
        [Parameter(Mandatory = $false)][switch]$NonInteractive
    )

    if ([string]::IsNullOrEmpty($BaseRef)) {
        Write-Host "ℹ️  Post-release upstream-dependency scan skipped (no base ref)." -ForegroundColor DarkGray
        return
    }

    $isInteractive = (-not $NonInteractive) -and (Test-InteractiveSession)

    $declined = [System.Collections.Generic.HashSet[string]]::new()

    # Termination bound: number of published workspace crates. The dep graph is
    # a DAG, so each iteration either grows ($declined ∪ release-set)
    # monotonically or terminates.
    $maxIterations = @(Get-WorkspaceCrates -repoRoot $RepoRoot | Where-Object { $_.Published }).Count
    if ($maxIterations -lt 1) { $maxIterations = 1 }

    # Save/restore the temp-diff-paths tracking list so a re-entry into this
    # function (current callers never re-enter, but the helper API allows it)
    # cannot clobber an outer run's list.
    $prevTempPaths = $script:TempPackageDiffPaths
    $script:TempPackageDiffPaths = [System.Collections.Generic.List[string]]::new()

    try {
        $firstIter = $true
        for ($iter = 0; $iter -lt $maxIterations; $iter++) {
            Invalidate-WorkspaceMetadataCache

            # The BFS below re-walks `cargo metadata`, runs git diffs against
            # each crate's last-release baseline, and reapplies the
            # declined/cascade filters — on a sizeable workspace this can
            # take several seconds. Show a status line so the user knows the
            # script hasn't hung between their menu choice and the next
            # prompt. Skip the indicator in non-interactive runs where the
            # output is just log noise.
            if ($isInteractive) {
                Write-Host ''
                Write-Host '🔍 Analyzing packages for unreleased modifications...' -ForegroundColor Cyan
            }

            $queue = @(
                @(Get-UnreleasedModifiedDependencies -RepoRoot $RepoRoot -BaseRef $BaseRef) |
                    Where-Object { -not $declined.Contains($_.Folder) }
            )

            if ($queue.Count -eq 0) {
                if ($firstIter) {
                    Write-Host ""
                    Write-Host "✅ No modified-but-unreleased upstream workspace dependencies detected." -ForegroundColor Green
                }
                return
            }

            if (-not $isInteractive) {
                # Non-interactive parity: emit the full pending list once and bail,
                # marking everything as declined. The reviewer-facing comment from
                # check-unreleased-dependencies.ps1 will flag the same set.
                Write-Host ""
                Write-Host '⚠️  The following workspace crates have unreleased modifications (changes newer than their last `version =` / `publish =` commit) and are NOT part of this release:' -ForegroundColor Yellow
                foreach ($finding in $queue) {
                    Write-Host "  • $($finding.Folder)" -ForegroundColor Yellow
                    Write-Host '      potentially affected dependency chains:' -ForegroundColor DarkGray
                    foreach ($chain in $finding.DependencyChains) {
                        Write-Host "        $($chain -join ' -> ')" -ForegroundColor DarkGray
                    }
                }
                Write-Warning "Non-interactive session: leaving the above crates unreleased. Reviewer should confirm the changes are immaterial."
                foreach ($finding in $queue) { [void]$declined.Add($finding.Folder) }
                return
            }

            $firstIter = $false

            # Process one finding per outer iteration. Cascade-bumped findings
            # naturally drop out of the next iteration's queue because the
            # cascade commits their version bumps, so they no longer appear
            # in the modified-but-unreleased set on the next BFS snapshot.
            $next       = $queue[0]
            $remaining  = $queue.Count - 1
            $decision   = Get-PackageReleaseDecision -Finding $next -RemainingCount $remaining -RepoRoot $RepoRoot

            if ($decision.Action -eq 'ignore') {
                Write-Host "  Leaving '$($next.Folder)' unreleased; reviewer should confirm the change is immaterial." -ForegroundColor DarkGray
                [void]$declined.Add($next.Folder)
                continue
            }

            Write-Host ""
            Write-Host "🚀 Releasing '$($next.Folder)' as $($decision.Action)..." -ForegroundColor Cyan
            $nestedReleases = @(Invoke-ReleaseFlow -CrateName $next.Folder -Bump $decision.Action `
                -RepoRoot $RepoRoot -RootCargoToml $RootCargoToml -PrBaseUrl $PrBaseUrl -BaseRef $BaseRef)

            # Merge nested release records into the running set. A crate may already
            # appear (e.g., it was a downstream cascade target of the initial release)
            # and the nested cascade may have upgraded it further — preserve the
            # original OldVersion (the pre-PR baseline) and adopt the latest NewVersion
            # so Show-ReleaseSummary and the final commit message reflect on-disk state.
            foreach ($r in $nestedReleases) {
                # If cascade pulled in a package the user previously chose to ignore,
                # surface that so they're not confused why it appears in the release
                # summary, and update $declined to reflect reality.
                if ($declined.Contains($r.Crate)) {
                    Write-Host "ℹ️  Previously ignored package '$($r.Crate)' was cascade-bumped because '$($next.Folder)' was released." -ForegroundColor DarkCyan
                    [void]$declined.Remove($r.Crate)
                }

                $existing = $ReleasesRef.Value | Where-Object { $_.Crate -eq $r.Crate } | Select-Object -First 1
                if ($null -eq $existing) {
                    $ReleasesRef.Value += $r
                } else {
                    $existing.NewVersion = $r.NewVersion
                }
            }
        }

        Write-Warning "Post-release dependency scan reached its iteration cap ($maxIterations); aborting further prompts."
    } finally {
        foreach ($p in $script:TempPackageDiffPaths) {
            try {
                if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Force -ErrorAction Stop }
            } catch {
                Write-Warning "Could not delete temp diff file '$p': $_"
            }
        }
        $script:TempPackageDiffPaths = $prevTempPaths
    }
}

# Wrapper around the post-release workspace consistency check. Extracted to a
# function so tests can mock it (the real call requires cargo + a fully synced
# workspace, which is impractical inside Pester scenarios).
function Invoke-WorkspaceCheck {
    param([string]$RepoRoot)

    Write-Host ""
    Write-Host "🔍 Running workspace cargo check..." -ForegroundColor Cyan

    Push-Location $RepoRoot
    try {
        cargo check --workspace --quiet | Write-Host
        if ($LASTEXITCODE -ne 0) {
            Write-Error "Workspace 'cargo check' failed after version updates. Please verify the changes." -ErrorAction Stop
        }
    } finally {
        Pop-Location
    }
}

# Translates a semantic -Change value into the internal (Bump, Version) tuple
# the rest of the release flow expects. The script's user-facing vocabulary
# is intent-based (Breaking / NonBreaking / Patch / 1.0) because that's how
# releasers reason about the change, but every layer below Invoke-ReleaseMain
# still uses Cargo's numeric major/minor/patch terms because changelogs,
# Cargo.toml, and commit messages are concrete version transitions.
#
# Returned object has exactly one of { Bump, Version } populated:
#   - Breaking    → Bump='major'    Version=''
#   - NonBreaking → Bump='minor'    Version=''
#   - Patch       → Bump='patch'    Version=''
#   - 1.0         → Bump=''         Version='1.0.0'  (the one-time graduation)
#
# The 1.0 graduation throws when invoked on a package that's already at or
# beyond 1.0.0 — the caller is expected to surface the message to the user
# and exit. This keeps the lifecycle event idempotent against accidental
# re-invocation (you can't graduate to 1.0 twice).
function Resolve-ReleaseSpecFromChange {
    param(
        [Parameter(Mandatory = $true)][ValidateSet('Breaking', 'NonBreaking', 'Patch', '1.0')][string]$Change,
        [Parameter(Mandatory = $true)][string]$CurrentVersion
    )

    switch ($Change) {
        'Breaking'    { return [pscustomobject]@{ Bump = 'major'; Version = '' } }
        'NonBreaking' { return [pscustomobject]@{ Bump = 'minor'; Version = '' } }
        'Patch'       { return [pscustomobject]@{ Bump = 'patch'; Version = '' } }
        '1.0' {
            # Force array context — see Compare-SemanticVersions for the rationale.
            $parts = @($CurrentVersion.Split('.') | ForEach-Object { [int]$_ })
            while ($parts.Count -lt 3) { $parts += 0 }
            if ($parts[0] -ge 1) {
                throw "The '-Change 1.0' option is for the one-time 0.x → 1.0.0 graduation event. Current version '$CurrentVersion' is already at 1.x or higher; use '-Change Breaking' instead."
            }
            return [pscustomobject]@{ Bump = ''; Version = '1.0.0' }
        }
    }
}

# Top-level entry point. Encapsulates input validation, pre-flight checks, git
# remote detection, base-ref resolution, the actual release workflow, and the
# post-release workspace check. Returns the array of release records (so tests
# can assert on them); also prints the summary and final message.
#
# This function exists so Pester tests can drive the full flow in-process
# without spawning a child PowerShell — see scripts/tests/Pester/scenarios/.
function Invoke-ReleaseMain {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$CrateName,
        [Parameter(Mandatory = $false)][string]$Version,
        [Parameter(Mandatory = $false)][ValidateSet('Breaking', 'NonBreaking', 'Patch', '1.0')][string]$Change,
        [Parameter(Mandatory = $false)][string]$BaseRef = 'origin/main',
        [Parameter(Mandatory = $false)][switch]$NonInteractive
    )

    # 1. INPUT VALIDATION
    if (-not (Test-ValidCrateName -crateName $CrateName)) {
        Write-Error "Invalid crate name '$CrateName'. Crate names must contain only letters, numbers, hyphens, and underscores, cannot start or end with hyphen, and must be 64 characters or less."
        Exit 1
    }

    if (-not [string]::IsNullOrEmpty($Version) -and -not [string]::IsNullOrEmpty($Change)) {
        Write-Error "The --version and --change options are mutually exclusive. Please specify only one."
        Exit 1
    }

    if (-not (Test-ValidVersion -version $Version)) {
        Write-Error "Invalid version format '$Version'. Version must follow semantic versioning format (e.g., '1.2.3')."
        Exit 1
    }

    # 2. PRE-FLIGHT CHECKS
    if (-not (Test-CommandExists -command "git")) {
        Write-Error "Git is not installed or not found in your PATH."
        Exit 1
    }

    $repoRoot = Get-Location
    if (-not (Test-Path (Join-Path $repoRoot ".git"))) {
        Write-Error "This script must be run from the root of a Git repository."
        Exit 1
    }

    $crateFolder = Join-Path $repoRoot 'crates' $CrateName
    if (-not (Test-Path $crateFolder)) {
        Write-Error "Crate folder not found at '$crateFolder'. Please check the CrateName."
        Exit 1
    }

    # 3. DETERMINE GITHUB REPO URL
    $prBaseUrl = $null
    $remoteUrl = Invoke-Git -Arguments @('remote', 'get-url', 'origin') -RepoRoot $repoRoot.Path -AllowFailure
    if ($remoteUrl -and $remoteUrl -match $script:GitHubRepoRegex) {
        $repoIdentifier = $matches[1] -replace '\.git$', ''
        $prBaseUrl = "https://github.com/$repoIdentifier/pull"
    } else {
        Write-Warning "Could not determine GitHub repository from remote 'origin'. Links will not be generated."
    }

    # 4. DEFINE FILE PATHS
    $crateCargoToml = Join-Path $crateFolder "Cargo.toml"
    $rootCargoToml = Join-Path $repoRoot "Cargo.toml"

    if ((-not (Test-Path $crateCargoToml)) -or (-not (Test-Path $rootCargoToml))) {
        Write-Error "Could not find Cargo.toml file in the crate folder or repository root."
        Exit 1
    }

    # 5. RESOLVE BASE REF (best-effort fetch + validate)
    # Done before -Change / -Version validation so we can detect cross-invocation
    # pending releases and make those validations base-relative (otherwise a
    # re-invocation on an already-pending package — e.g. `-Change 1.0` on a
    # package already pending at v1.0.0, or `-Version X` where X equals the
    # current pending version — would spuriously fail the on-disk comparison).
    $resolvedBaseRef = $BaseRef
    if (-not [string]::IsNullOrEmpty($resolvedBaseRef)) {
        if ($resolvedBaseRef -match '^origin/(.+)$') {
            $branch = $matches[1]
            try {
                Invoke-Git -Arguments @('fetch', '--no-tags', 'origin', "+refs/heads/${branch}:refs/remotes/origin/${branch}") -RepoRoot $repoRoot.Path | Out-Null
            } catch {
                Write-Warning "git fetch for '$resolvedBaseRef' failed: $_"
            }
        }
        if (-not (Test-GitRef -Ref $resolvedBaseRef -RepoRoot $repoRoot.Path)) {
            Write-Warning "Could not resolve base ref '$resolvedBaseRef'; post-release upstream-dependency scan will be skipped."
            $resolvedBaseRef = ''
        }
    }

    # 6. ANNOUNCE PENDING UNCOMMITTED RELEASES
    # Helps the user notice prior `release-crate.ps1` runs whose results haven't
    # been committed yet, so they understand why the analysis treats those
    # packages as already-bumped. Skipped silently when BaseRef is unresolved
    # (without a base, "pending" has no meaning).
    $pendingReleases = @()
    if (-not [string]::IsNullOrEmpty($resolvedBaseRef)) {
        $pendingReleases = @(Get-PendingReleases -RepoRoot $repoRoot.Path -BaseRef $resolvedBaseRef)
        if ($pendingReleases.Count -gt 0) {
            Write-Host ""
            Write-Host (Format-PendingReleasesAnnouncement -Pending $pendingReleases) -ForegroundColor DarkGray
        }
    }

    # Determine whether the primary target is among the pending set. When it is,
    # downstream validation uses BaseVersion (not on-disk current) as the anchor
    # so this invocation is base-relative — mirroring Invoke-CascadeStep's
    # treatment of already-bumped dependents.
    $primaryPending = $pendingReleases | Where-Object { $_.Folder -eq $CrateName } | Select-Object -First 1

    # 7. RESOLVE -Change INTO INTERNAL ($Bump, $Version)
    # The CLI surface uses semantic vocabulary (Breaking / NonBreaking / Patch / 1.0)
    # because that's how releasers reason about the change. Below this point the
    # release flow continues in Cargo's numeric major/minor/patch terms because
    # changelogs, Cargo.toml, and commit messages are concrete version transitions.
    # The 1.0 graduation translates into an explicit -Version 1.0.0; everything
    # else translates into the matching -Bump kind.
    $bump = ''
    if (-not [string]::IsNullOrEmpty($Change)) {
        # Use BaseVersion (when pending) so a re-invocation of `-Change 1.0` on
        # an already-graduated pending package idempotently no-ops instead of
        # throwing "already at 1.x" from on-disk inspection. Only the 1.0
        # branch of Resolve-ReleaseSpecFromChange consults this version.
        $versionForChangeCheck = if ($null -ne $primaryPending) {
            $primaryPending.BaseVersion
        } else {
            Get-CurrentVersion -cargoTomlPath $crateCargoToml
        }
        if ($null -eq $versionForChangeCheck) {
            Write-Error "Failed to get current version for comparison. Aborting."
            Exit 1
        }
        try {
            $spec = Resolve-ReleaseSpecFromChange -Change $Change -CurrentVersion $versionForChangeCheck
        } catch {
            Write-Error $_.Exception.Message
            Exit 1
        }
        $bump = $spec.Bump
        if (-not [string]::IsNullOrEmpty($spec.Version)) {
            $Version = $spec.Version
        }
    }

    # 8. VERSION COMPARISON VALIDATION
    if (-not [string]::IsNullOrEmpty($Version)) {
        # Anchor is BaseVersion (when pending) so an idempotent re-invocation
        # passing the SAME -Version as the current pending version is accepted
        # (it satisfies `Version > BaseVersion`). The actual three-way
        # comparison against on-disk current — equal=no-op, lower=error,
        # higher=upgrade-error — happens in Invoke-ReleaseFlow.
        $versionAnchor = if ($null -ne $primaryPending) {
            $primaryPending.BaseVersion
        } else {
            Get-CurrentVersion -cargoTomlPath $crateCargoToml
        }
        if ($null -eq $versionAnchor) {
            Write-Error "Failed to get current version for comparison. Aborting."
            Exit 1
        }

        $versionComparison = Compare-SemanticVersions -version1 $Version -version2 $versionAnchor
        if ($versionComparison -le 0) {
            $anchorLabel = if ($null -ne $primaryPending) { "base version '$versionAnchor'" } else { "current version '$versionAnchor'" }
            Write-Error "Specified version '$Version' must be greater than $anchorLabel. Please specify a higher version number."
            Exit 1
        }
    }

    # 9. EXECUTE WORKFLOW
    try {
        $releases = @(Invoke-ReleaseFlow -CrateName $CrateName -Version $Version -Bump $bump `
            -RepoRoot $repoRoot.Path -RootCargoToml $rootCargoToml -PrBaseUrl $prBaseUrl -BaseRef $resolvedBaseRef)

        # Scan for modified-but-unreleased upstream deps and prompt the user. Newly-released
        # crates are appended to $releases via the [ref].
        Invoke-PostReleaseDepScan -RepoRoot $repoRoot.Path -BaseRef $resolvedBaseRef `
            -ReleasesRef ([ref]$releases) -RootCargoToml $rootCargoToml -PrBaseUrl $prBaseUrl `
            -NonInteractive:$NonInteractive

        Invoke-WorkspaceCheck -RepoRoot $repoRoot.Path

        Show-ReleaseSummary -releases $releases
        Show-FinalMessage -CrateName $CrateName -Releases $releases

        return ,$releases
    }
    catch {
        Write-Error "Script failed: $_"
        Exit 1
    }
}
