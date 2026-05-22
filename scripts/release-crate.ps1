# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates the version of a Rust crate and generates a CHANGELOG.md file based on git history.

.DESCRIPTION
    This script automates the full release of a Rust crate in a workspace repository:
    1. Version Bump: Automatically increment the version (major, minor, or patch) or set a specific
       version. Cargo's 0.x.y SemVer rules are honored — for `0.x.y` crates a `major` bump becomes
       `0.(x+1).0` and both `minor` and `patch` map to bumping `y`.
    2. Cascade: Every workspace crate that depends on the target via `[dependencies]` or
       `[build-dependencies]` (transitively) is also bumped. The bump kind applied to each
       dependent is informed by `[package.metadata.cargo_check_external_types]` AND by whether
       the target's bump is SemVer-incompatible under Cargo's rules:
         * If the dependent exposes any type rooted at the bumped crate in its public API
           (or does not declare allowed_external_types at all), the dependent gets a `major`
           bump when the target's bump is breaking (e.g. `0.0.x → 0.0.(x+1)`, `0.x.y → 0.(x+1).0`,
           `1.x → 2.0`); otherwise the same kind as the target. This ensures the dependent's
           own version increment reflects the breaking change in its public API surface.
         * Otherwise, the dependent only uses the bumped crate internally, and a `patch` bump
           is applied: enough to refresh the workspace-pinned version, but without overstating
           the change to downstream consumers.
       Dev-only dependents are skipped — they automatically pick up the new workspace version.
    3. Changelog Generation: A CHANGELOG.md entry is generated for the target and every cascaded
       dependent. Cascaded crates that have no other commits since their last release get a single
       `Now requires <new-version> of \`<target>\`` entry under `🔧 Maintenance` (or `⚠️ Breaking`
       for major bumps).

    By default, if neither --version nor --bump is specified, the script will perform a minor bump
    of the target crate (e.g., 1.2.3 -> 1.3.0, or 0.3.3 -> 0.3.4 for `0.x.y` crates).

.PARAMETER CrateName
    The name of the crate to release. This should match the folder name inside the 'crates' directory.

.PARAMETER Version
    [Optional] The specific version to set (e.g., "1.2.3"). Can be specified with --version or -v.
    This parameter is mutually exclusive with --bump.

.PARAMETER Bump
    [Optional] The version component to bump: 'major', 'minor', or 'patch'. Can be specified with --bump or -b.
    - major: Increments the major version and resets minor and patch to 0 (e.g., 1.2.3 -> 2.0.0)
    - minor: Increments the minor version and resets patch to 0 (e.g., 1.2.3 -> 1.3.0)
    - patch: Increments the patch version (e.g., 1.2.3 -> 1.2.4)
    This parameter is mutually exclusive with --version.

.EXAMPLE
    # Increment the minor version for 'my-crate' (default behavior)
    .\release-crate.ps1 "my-crate"

.EXAMPLE
    # Set a specific version for 'my-crate'
    .\release-crate.ps1 my-crate --version "2.5.0"

.EXAMPLE
    # Bump the major version for 'my-crate'
    .\release-crate.ps1 my-crate --bump major

.EXAMPLE
    # Bump the patch version for 'my-crate'
    .\release-crate.ps1 my-crate -b patch
#>
[CmdletBinding()]
param(
    # Non-mandatory so test harnesses can dot-source this script (with
    # $env:OXI_RELEASE_CRATE_NOEXEC = '1') to access the helper functions
    # defined within without executing the entrypoint. Production callers
    # must still supply -CrateName; Invoke-ReleaseMain validates it.
    [Parameter(Mandatory = $false, Position = 0)]
    [string]$CrateName,

    [Parameter(Mandatory = $false)]
    [Alias('v')]
    [string]$Version,

    [Parameter(Mandatory = $false)]
    [Alias('b')]
    [ValidateSet('major', 'minor', 'patch')]
    [string]$Bump,

    # Base ref used to identify the release set (crates whose `version =` differs
    # between this ref and HEAD) for the post-release upstream-dependency scan.
    # The modification baseline for each upstream dep is per-crate (the dep's own
    # last `version =` / `publish =` commit), not this ref. Default is
    # 'origin/main' (best-effort fetched before use). Pass an empty string to skip
    # the scan entirely.
    [Parameter(Mandatory = $false)]
    [string]$BaseRef = 'origin/main',

    # Suppress all interactive prompts (decline-by-default for the upstream-dependency
    # scan). Auto-enabled in CI / when stdin is redirected; this switch is the explicit
    # override for scripted callers.
    [Parameter(Mandatory = $false)]
    [switch]$NonInteractive
)

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

# --- DOT-SOURCE SHARED LIBRARY ---
#
# scripts/lib/releasing.ps1 owns the reusable building blocks used by both this
# script and scripts/check-unreleased-dependencies.ps1:
#   - Compiled regex patterns ($script:ConventionalCommitRegex, $script:PrReferenceRegex,
#     $script:SemanticVersionRegex, $script:CargoVersionRegex, $script:GitHubRepoRegex,
#     $script:RegexEscapeRegex).
#   - Safe git invocation (Invoke-Git) and ref validation (Test-GitRef).
#   - SemVer arithmetic (Compare-SemanticVersions, Get-NextVersion, Get-BumpKindFromVersions,
#     Test-IsBreakingChange) and crate-version readers (Get-CurrentVersion,
#     Get-CrateVersionFromRef).
#   - Workspace metadata (Get-WorkspaceMetadata, Get-WorkspaceCrates,
#     Invalidate-WorkspaceMetadataCache, Test-CrateExposesTarget, Get-AllTransitiveDependents).
#   - Modified-but-unreleased dependency analysis (Get-CratesWithUnreleasedChanges,
#     Get-CratesWithVersionBumps, Get-UnreleasedModifiedDependencies).
. "$PSScriptRoot/lib/releasing.ps1"

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
    (Get-Content $crateCargoToml -Raw) -replace '(?<=version\s*=\s*")[^"]+', $newVersion | Set-Content $crateCargoToml -NoNewline

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
    # dependencies was bumped. Format follows existing convention in the repo
    # (e.g. crates/cachet/CHANGELOG.md):
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
                $headerMatch = [regex]::Match($existingContent, $headerPattern)
                $insertPosition = $headerMatch.Index + $headerMatch.Length
                $newContent = $existingContent.Substring(0, $insertPosition) +
                              ($newVersionSection -join "`n") + "`n" +
                              $existingContent.Substring($insertPosition)
                $newContent | Set-Content $changelogFile -NoNewline
                Write-Host "✅ Changelog updated at '$changelogFile'."
                return
            }
        }
    }

    # If no existing changelog or couldn't parse it, create a new one
    $changelogContent = @("# Changelog", "")
    $changelogContent += $newVersionSection
    $changelogContent | Out-File -FilePath $changelogFile -Encoding utf8
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
        [string]$crateName,
        [string]$newVersion
    )

    Write-Host "---" -ForegroundColor Green
    Write-Host "🎉 Success! Next steps:" -ForegroundColor Green
    Write-Host "1. Review the changes in the updated files." -ForegroundColor Green
    Write-Host "2. Commit the changes and push the changes:" -ForegroundColor Green
    Write-Host "   git add ." -ForegroundColor DarkGray
    Write-Host "   git commit -m `"feat($crateName): release v$newVersion`"" -ForegroundColor DarkGray
    Write-Host "   git push origin mybranch" -ForegroundColor DarkGray
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

    Set-Content -LiteralPath $ChangelogFile -Value $new -Encoding utf8
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

    $oldVersion = Get-CurrentVersion -cargoTomlPath $crateCargoToml

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

    $releases = @(
        [pscustomobject]@{ Crate = $CrateName; OldVersion = $oldVersion; NewVersion = $newVersion }
    )

    $targetIsBreaking = Test-IsBreakingChange -oldVersion $oldVersion -bump $cascadeBump
    $exposingCascadeBump = if ($targetIsBreaking) { 'major' } else { $cascadeBump }

    $dependents = @(Get-AllTransitiveDependents -crateName $CrateName -repoRoot $RepoRoot)
    if ($dependents.Count -gt 0) {
        Write-Host ""
        Write-Host "🔗 Cascading $exposingCascadeBump bump (or patch where the dependent does not expose '$CrateName' types) to $($dependents.Count) crates: $($dependents -join ', ')" -ForegroundColor Cyan

        $allCrates = Get-WorkspaceCrates -repoRoot $RepoRoot
        $targetCrate = $allCrates | Where-Object { $_.Folder -eq $CrateName -or $_.Name -eq $CrateName } | Select-Object -First 1
        $targetPackageName = if ($null -ne $targetCrate) { $targetCrate.Name } else { $CrateName }

        foreach ($dependent in $dependents) {
            $depCrate = $allCrates | Where-Object { $_.Folder -eq $dependent } | Select-Object -First 1
            $exposes = if ($null -ne $depCrate) {
                Test-CrateExposesTarget -dependent $depCrate -targetPackageName $targetPackageName
            } else { $true }

            $depBump = if ($exposes) { $exposingCascadeBump } else { 'patch' }
            $exposureNote = if ($exposes) { 'exposes target in public API' } else { 'internal use only' }
            Write-Host "  • $dependent -> $depBump ($exposureNote)" -ForegroundColor DarkCyan

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

    $declined  = [System.Collections.Generic.HashSet[string]]::new()
    $prompted  = [System.Collections.Generic.HashSet[string]]::new()
    # Tracks crate folders currently in the release set so a foreach iteration whose
    # target was cascade-bumped by a prior accepted release within the same loop is
    # skipped rather than mis-prompted. Seeded from the on-disk state and grown after
    # every nested Invoke-ReleaseFlow.
    $currentReleaseSet = Get-CratesWithVersionBumps -RepoRoot $RepoRoot -BaseRef $BaseRef

    # Termination bound: number of published workspace crates. The dep graph is a DAG,
    # so each iteration either grows ($prompted ∪ release-set) monotonically or terminates.
    $maxIterations = @(Get-WorkspaceCrates -repoRoot $RepoRoot | Where-Object { $_.Published }).Count
    if ($maxIterations -lt 1) { $maxIterations = 1 }

    for ($iter = 0; $iter -lt $maxIterations; $iter++) {
        Invalidate-WorkspaceMetadataCache
        $findings = @(Get-UnreleasedModifiedDependencies -RepoRoot $RepoRoot -BaseRef $BaseRef)

        $new = @($findings | Where-Object { -not $prompted.Contains($_.Folder) -and -not $declined.Contains($_.Folder) })
        if ($new.Count -eq 0) {
            if ($iter -eq 0) {
                Write-Host ""
                Write-Host "✅ No modified-but-unreleased upstream workspace dependencies detected." -ForegroundColor Green
            }
            return
        }

        Write-Host ""
        Write-Host '⚠️  The following workspace crates have unreleased modifications (changes newer than their last `version =` / `publish =` commit) and are NOT part of this release:' -ForegroundColor Yellow
        foreach ($finding in $new) {
            Write-Host "  • $($finding.Folder) — $($finding.ChangedFileCount) files changed" -ForegroundColor Yellow
            foreach ($chain in $finding.DependencyChains) {
                Write-Host "      pulled in by: $($chain -join ' -> ')" -ForegroundColor DarkGray
            }
        }

        if (-not $isInteractive) {
            Write-Warning "Non-interactive session: leaving the above crates unreleased. Reviewer should confirm the changes are immaterial."
            foreach ($finding in $new) {
                [void]$prompted.Add($finding.Folder)
                [void]$declined.Add($finding.Folder)
            }
            return
        }

        foreach ($finding in $new) {
            $folder = $finding.Folder
            [void]$prompted.Add($folder)

            # The list above was rendered from this iteration's pre-loop snapshot of
            # $new. A prior accepted release within the same foreach may have
            # cascade-bumped $folder into the release set since then; in that case
            # the release decision has already been made and prompting again would
            # mislead the user (saying "n" would print "Leaving X unreleased" even
            # though X IS being released as a cascade).
            if ($currentReleaseSet.Contains($folder)) {
                $existing = $ReleasesRef.Value | Where-Object { $_.Crate -eq $folder } | Select-Object -First 1
                $versionSuffix = if ($null -ne $existing) { " (now at $($existing.NewVersion))" } else { '' }
                Write-Host "  • '$folder' was cascade-bumped by a prior release in this run$versionSuffix — skipping prompt (already in release set)." -ForegroundColor DarkGray
                continue
            }

            $answer = (Read-Host "Release '$folder' too? [y/N]").Trim().ToLowerInvariant()
            if ($answer -ne 'y' -and $answer -ne 'yes') {
                Write-Host "  Leaving '$folder' unreleased; reviewer should confirm the change is immaterial." -ForegroundColor DarkGray
                [void]$declined.Add($folder)
                continue
            }

            $bumpRaw = (Read-Host "Bump kind for '$folder'? [P]atch / [M]inor / (MA)jor (default: minor)").Trim().ToLowerInvariant()
            $bumpKind = switch ($bumpRaw) {
                ''        { 'minor' }
                'p'       { 'patch' }
                'patch'   { 'patch' }
                'm'       { 'minor' }
                'minor'   { 'minor' }
                'ma'      { 'major' }
                'major'   { 'major' }
                default   {
                    Write-Warning "Unrecognized bump '$bumpRaw'; defaulting to 'minor'."
                    'minor'
                }
            }

            Write-Host ""
            Write-Host "🚀 Releasing '$folder' as $bumpKind..." -ForegroundColor Cyan
            $nestedReleases = @(Invoke-ReleaseFlow -CrateName $folder -Bump $bumpKind `
                -RepoRoot $RepoRoot -RootCargoToml $RootCargoToml -PrBaseUrl $PrBaseUrl -BaseRef $BaseRef)

            # Merge nested release records into the running set. A crate may already
            # appear (e.g., it was a downstream cascade target of the initial release)
            # and the nested cascade may have upgraded it further — preserve the
            # original OldVersion (the pre-PR baseline) and adopt the latest NewVersion
            # so Show-ReleaseSummary and the final commit message reflect on-disk state.
            foreach ($r in $nestedReleases) {
                # Track every crate touched by the nested release (target + cascade
                # dependents) so subsequent foreach iterations in this $new can skip
                # crates that are now in the release set.
                [void]$currentReleaseSet.Add($r.Crate)

                $existing = $ReleasesRef.Value | Where-Object { $_.Crate -eq $r.Crate } | Select-Object -First 1
                if ($null -eq $existing) {
                    $ReleasesRef.Value += $r
                } else {
                    $existing.NewVersion = $r.NewVersion
                }
            }
        }
    }

    Write-Warning "Post-release dependency scan reached its iteration cap ($maxIterations); aborting further prompts."
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
        [Parameter(Mandatory = $false)][ValidateSet('major', 'minor', 'patch')][string]$Bump,
        [Parameter(Mandatory = $false)][string]$BaseRef = 'origin/main',
        [Parameter(Mandatory = $false)][switch]$NonInteractive
    )

    # 1. INPUT VALIDATION
    if (-not (Test-ValidCrateName -crateName $CrateName)) {
        Write-Error "Invalid crate name '$CrateName'. Crate names must contain only letters, numbers, hyphens, and underscores, cannot start or end with hyphen, and must be 64 characters or less."
        Exit 1
    }

    if (-not [string]::IsNullOrEmpty($Version) -and -not [string]::IsNullOrEmpty($Bump)) {
        Write-Error "The --version and --bump options are mutually exclusive. Please specify only one."
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

    # 5. VERSION COMPARISON VALIDATION
    if (-not [string]::IsNullOrEmpty($Version)) {
        $currentVersion = Get-CurrentVersion -cargoTomlPath $crateCargoToml
        if ($null -eq $currentVersion) {
            Write-Error "Failed to get current version for comparison. Aborting."
            Exit 1
        }

        $versionComparison = Compare-SemanticVersions -version1 $Version -version2 $currentVersion
        if ($versionComparison -le 0) {
            Write-Error "Specified version '$Version' must be greater than current version '$currentVersion'. Please specify a higher version number."
            Exit 1
        }
    }

    # 6. RESOLVE BASE REF (best-effort fetch + validate)
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

    # 7. EXECUTE WORKFLOW
    try {
        $releases = @(Invoke-ReleaseFlow -CrateName $CrateName -Version $Version -Bump $Bump `
            -RepoRoot $repoRoot.Path -RootCargoToml $rootCargoToml -PrBaseUrl $prBaseUrl -BaseRef $resolvedBaseRef)

        # Scan for modified-but-unreleased upstream deps and prompt the user. Newly-released
        # crates are appended to $releases via the [ref].
        Invoke-PostReleaseDepScan -RepoRoot $repoRoot.Path -BaseRef $resolvedBaseRef `
            -ReleasesRef ([ref]$releases) -RootCargoToml $rootCargoToml -PrBaseUrl $prBaseUrl `
            -NonInteractive:$NonInteractive

        Invoke-WorkspaceCheck -RepoRoot $repoRoot.Path

        $primary = $releases | Where-Object { $_.Crate -eq $CrateName } | Select-Object -First 1
        $primaryNewVersion = if ($null -ne $primary) { $primary.NewVersion } else { '' }

        Show-ReleaseSummary -releases $releases
        Show-FinalMessage -crateName $CrateName -newVersion $primaryNewVersion

        return ,$releases
    }
    catch {
        Write-Error "Script failed: $_"
        Exit 1
    }
}

# --- SCRIPT EXECUTION ---

# Test harnesses dot-source this script with $env:OXI_RELEASE_CRATE_NOEXEC = '1'
# to access internal helpers without running the release flow. Production
# callers leave the variable unset and the entrypoint runs as usual.
if ($env:OXI_RELEASE_CRATE_NOEXEC -ne '1') {
    Invoke-ReleaseMain -CrateName $CrateName -Version $Version -Bump $Bump -BaseRef $BaseRef -NonInteractive:$NonInteractive | Out-Null
}
