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
       `[build-dependencies]` (transitively) is bumped with the same bump kind. Dev-only dependents
       are skipped — they automatically pick up the new workspace version. This mirrors the
       guidance in `.github/prompts/bump-crate-version.prompt.md` and prevents the publish-time
       inconsistencies that would otherwise occur when a core crate is bumped in isolation.
    3. Changelog Generation: A CHANGELOG.md entry is generated for the target and every cascaded
       dependent. Cascaded crates that have no other commits since their last release get a single
       `bump \`<target>\` to <new-version>` entry under `🔧 Maintenance` (or `⚠️ Breaking` for
       major bumps).

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
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$CrateName,

    [Parameter(Mandatory = $false)]
    [Alias('v')]
    [string]$Version,

    [Parameter(Mandatory = $false)]
    [Alias('b')]
    [ValidateSet('major', 'minor', 'patch')]
    [string]$Bump
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

# --- COMPILED REGEX PATTERNS ---

# Pattern for conventional commit format: type(scope)!: description (! indicates breaking change)
$script:ConventionalCommitRegex = [regex]'^(\w+)(?:\(.*\))?(!)?:\s*(.*)'

# Pattern for PR references: (#123)
$script:PrReferenceRegex = [regex]'\s*(\(#(\d+)\))$'

# Pattern for semantic version format: major.minor.patch
$script:SemanticVersionRegex = [regex]'^\d+\.\d+\.\d+$'

# Pattern for extracting version from Cargo.toml: version = "x.y.z"
$script:CargoVersionRegex = [regex]'(?<=version\s*=\s*")[^"]+'

# Pattern for GitHub repository URL matching
$script:GitHubRepoRegex = [regex]'github\.com[/:]([\w.-]+/[\w.-]+)'

# Pattern for regex metacharacters that need escaping
$script:RegexEscapeRegex = [regex]'([\\\.$\^\{\[\(\|\)\*\+\?\/])'

# --- HELPER FUNCTIONS ---

function Test-CommandExists {
    param([string]$Command)
    return $null -ne (Get-Command $Command -ErrorAction SilentlyContinue)
}

function Test-ValidCrateName {
    param([string]$crateName)
    # Validate crate name: must contain only letters, numbers, hyphens, and underscores
    # Must not start or end with hyphen, and must not be empty
    return $crateName -match '^[a-zA-Z0-9]([a-zA-Z0-9_-]*[a-zA-Z0-9])?$' -and $crateName.Length -le 64
}

function Test-ValidVersion {
    param([string]$version)
    if ([string]::IsNullOrEmpty($version)) {
        return $true  # Empty version is valid (will be auto-incremented)
    }
    return $script:SemanticVersionRegex.IsMatch($version)
}

function Compare-SemanticVersions {
    param(
        [string]$version1,
        [string]$version2
    )

    # Parse version strings into arrays of integers
    $v1Parts = $version1.Split('.') | ForEach-Object { [int]$_ }
    $v2Parts = $version2.Split('.') | ForEach-Object { [int]$_ }

    # Ensure both arrays have 3 elements (major, minor, patch)
    while ($v1Parts.Count -lt 3) { $v1Parts += 0 }
    while ($v2Parts.Count -lt 3) { $v2Parts += 0 }

    # Compare major, minor, patch in order
    for ($i = 0; $i -lt 3; $i++) {
        if ($v1Parts[$i] -gt $v2Parts[$i]) {
            return 1  # version1 > version2
        }
        elseif ($v1Parts[$i] -lt $v2Parts[$i]) {
            return -1  # version1 < version2
        }
    }

    return 0  # versions are equal
}

# Loads workspace package metadata once and caches it.
$script:CachedWorkspaceMetadata = $null
function Get-WorkspaceMetadata {
    param([string]$repoRoot)

    if ($null -ne $script:CachedWorkspaceMetadata) {
        return $script:CachedWorkspaceMetadata
    }

    $rootManifest = Join-Path $repoRoot "Cargo.toml"
    $metadataJson = cargo metadata --format-version=1 --no-deps --manifest-path $rootManifest
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Failed to run 'cargo metadata'." -ErrorAction Stop
    }

    $script:CachedWorkspaceMetadata = $metadataJson | ConvertFrom-Json
    return $script:CachedWorkspaceMetadata
}

# Returns information about all workspace crates as an array of objects with:
#   Name      - cargo package name
#   Folder    - folder name under crates/ (used as the script's CrateName argument)
#   Published - $true if the crate is published to crates.io
#   Deps      - array of normalized dependency names (kind 'normal' or 'build', not 'dev')
function Get-WorkspaceCrates {
    param([string]$repoRoot)

    $metadata = Get-WorkspaceMetadata -repoRoot $repoRoot
    $cratesDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "crates"))

    $crates = @()
    foreach ($package in $metadata.packages) {
        $manifestDir = [System.IO.Path]::GetFullPath((Split-Path $package.manifest_path -Parent))
        if (-not $manifestDir.StartsWith($cratesDir, [System.StringComparison]::OrdinalIgnoreCase)) {
            continue
        }

        $deps = @()
        foreach ($dep in $package.dependencies) {
            # Cascade follows normal and build deps; dev-deps automatically pick up the workspace
            # version on `cargo publish` and never need their own version bumped.
            if ($dep.kind -ne 'dev') {
                $deps += $dep.name.Replace('-', '_')
            }
        }

        $crates += [pscustomobject]@{
            Name      = $package.name
            Folder    = Split-Path $manifestDir -Leaf
            Published = -not ($null -ne $package.publish -and $package.publish.Count -eq 0)
            Deps      = $deps
        }
    }

    return $crates
}

# Computes the transitive set of published workspace crates that depend on the given target via
# [dependencies] or [build-dependencies]. Returns folder names suitable for indexing into crates/.
# The target itself is not included.
function Get-AllTransitiveDependents {
    param(
        [string]$crateName,
        [string]$repoRoot
    )

    $crates = Get-WorkspaceCrates -repoRoot $repoRoot

    # Resolve the user-facing crate identifier (folder name or package name) to the canonical
    # package name. This makes the cascade robust even if a crate's folder under crates/ ever
    # diverges from its [package].name in Cargo.toml.
    $targetCrate = $crates | Where-Object { $_.Folder -eq $crateName -or $_.Name -eq $crateName } | Select-Object -First 1
    if ($null -eq $targetCrate) {
        Write-Warning "Crate '$crateName' not found in workspace metadata; cannot compute dependents."
        return @()
    }
    $normalizedTarget = $targetCrate.Name.Replace('-', '_')

    # BFS over the reverse dependency graph.
    $toVisit = [System.Collections.Generic.Queue[string]]::new()
    $toVisit.Enqueue($normalizedTarget)
    $visited = [System.Collections.Generic.HashSet[string]]::new()
    [void]$visited.Add($normalizedTarget)

    $dependents = @()
    while ($toVisit.Count -gt 0) {
        $current = $toVisit.Dequeue()
        foreach ($candidate in $crates) {
            $candidateNorm = $candidate.Name.Replace('-', '_')
            if ($visited.Contains($candidateNorm)) {
                continue
            }
            if ($candidate.Deps -contains $current) {
                [void]$visited.Add($candidateNorm)
                $toVisit.Enqueue($candidateNorm)
                if ($candidate.Published) {
                    $dependents += $candidate.Folder
                }
            }
        }
    }

    # Wrap with @(...) at the call site to preserve array semantics for 0/1-element results.
    return $dependents
}

# Computes the next version for the given bump kind, honoring Cargo's 0.x.y SemVer rules:
#   - For x.y.z (x >= 1): major -> (x+1).0.0, minor -> x.(y+1).0, patch -> x.y.(z+1)
#   - For 0.x.y (x >= 1): major -> 0.(x+1).0, minor and patch -> 0.x.(y+1)
#   - For 0.0.x:           every bump -> 0.0.(x+1) (every change is breaking)
function Get-NextVersion {
    param(
        [string]$currentVersion,
        [ValidateSet('major', 'minor', 'patch')]
        [string]$bump
    )

    $parts = $currentVersion.Split('.') | ForEach-Object { [int]$_ }
    while ($parts.Count -lt 3) { $parts += 0 }

    if ($parts[0] -ge 1) {
        switch ($bump) {
            'major' { return "$($parts[0] + 1).0.0" }
            'minor' { return "$($parts[0]).$($parts[1] + 1).0" }
            'patch' { return "$($parts[0]).$($parts[1]).$($parts[2] + 1)" }
        }
    }
    elseif ($parts[1] -ge 1) {
        switch ($bump) {
            'major' { return "0.$($parts[1] + 1).0" }
            default { return "0.$($parts[1]).$($parts[2] + 1)" }
        }
    }
    else {
        return "0.0.$($parts[2] + 1)"
    }
}

# Infers a bump kind from the difference between two versions. Used when the caller passed an
# explicit --version so the cascade can apply a matching kind to dependents.
function Get-BumpKindFromVersions {
    param(
        [string]$oldVersion,
        [string]$newVersion
    )

    $oldParts = $oldVersion.Split('.') | ForEach-Object { [int]$_ }
    $newParts = $newVersion.Split('.') | ForEach-Object { [int]$_ }
    while ($oldParts.Count -lt 3) { $oldParts += 0 }
    while ($newParts.Count -lt 3) { $newParts += 0 }

    if ($oldParts[0] -ge 1) {
        if ($newParts[0] -ne $oldParts[0]) { return 'major' }
        if ($newParts[1] -ne $oldParts[1]) { return 'minor' }
        return 'patch'
    }
    if ($oldParts[1] -ge 1) {
        if ($newParts[1] -ne $oldParts[1]) { return 'major' }
        return 'patch'
    }
    return 'major'
}

function Get-CurrentVersion {
    param([string]$cargoTomlPath)

    if (-not (Test-Path $cargoTomlPath)) {
        Write-Error "Could not find Cargo.toml file at '$cargoTomlPath'." -ErrorAction Stop
    }

    $cargoContent = Get-Content $cargoTomlPath -Raw
    $currentVersionMatch = $script:CargoVersionRegex.Match($cargoContent)
    if (-not $currentVersionMatch.Success) {
        Write-Error "Could not determine current version from '$cargoTomlPath'." -ErrorAction Stop
    }

    return $currentVersionMatch.Value
}

function Invoke-GitCommand {
    param(
        [string]$command,
        [string]$errorMessage = "Git command failed"
    )

    $result = Invoke-Expression "git $command" 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Error "$errorMessage. Git command: git $command. Error: $result" -ErrorAction Stop
    }

    # Return empty array instead of null for commands with no output
    if ($null -eq $result -or $result.Count -eq 0) {
        return @()
    }

    return $result
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
        return ($str -replace $script:RegexEscapeRegex, '\\$1')
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

    $tags = Invoke-GitCommand -Command "tag --list `"$crateName-v*`"" -ErrorMessage "Failed to retrieve git tags"
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
    $rawCommits = Invoke-GitCommand -Command "log $range --pretty=format:`"%s`" -- `"$crateFolder`"" -ErrorMessage "Failed to retrieve git log for unreleased commits"
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
            Write-Warning "No relevant commits found to add to the changelog (all $($rawCommits.Count) commit(s) were filtered out)."
        }
        return
    }

    # Prepend a cascade entry when this crate is being bumped purely because one of its
    # dependencies was bumped. Format follows existing convention in the repo
    # (e.g. crates/cachet/CHANGELOG.md):
    #   - 🔧 Maintenance
    #     - bump `<target>` to <version>
    # If the same section header was already produced by Format-ConventionalCommits for this
    # release, the cascade bullet is merged into that existing section instead of creating a
    # duplicate header.
    if ($null -ne $cascadeReason) {
        $sectionHeader = if ($cascadeReason.Breaking) { '- ⚠️ Breaking' } else { '- 🔧 Maintenance' }
        $cascadeBullet = "  - bump ``$($cascadeReason.Target)`` to $($cascadeReason.Version)"

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

# --- SCRIPT EXECUTION ---

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
$remoteUrl = Invoke-GitCommand -command "remote get-url origin" -errorMessage "Failed to get remote URL"
if ($remoteUrl -and $remoteUrl -match $script:GitHubRepoRegex) {
    $repoIdentifier = $matches[1] -replace '\.git$', ''
    $prBaseUrl = "https://github.com/$repoIdentifier/pull"
} else {
    Write-Warning "Could not determine GitHub repository from remote 'origin'. Links will not be generated."
}

# 4. DEFINE FILE PATHS
$crateCargoToml = Join-Path $crateFolder "Cargo.toml"
$rootCargoToml = Join-Path $repoRoot "Cargo.toml"
$changelogFile = Join-Path $crateFolder "CHANGELOG.md"

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

# 6. EXECUTE WORKFLOW
try {
    $oldVersion = Get-CurrentVersion -cargoTomlPath $crateCargoToml

    $newVersion = Invoke-CrateRelease -crateName $CrateName -crateFolder $crateFolder `
        -crateCargoToml $crateCargoToml -rootCargoToml $rootCargoToml -changelogFile $changelogFile `
        -prBaseUrl $prBaseUrl -version $Version -bump $Bump

    # Determine the bump kind that was applied so we can cascade the same kind to dependents.
    # When the caller passed --version we infer it from old -> new.
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

    # Cascade the same bump kind to every transitive non-dev workspace dependent. This keeps
    # workspace-pinned versions consistent and prevents the publish-time failures described in
    # https://github.com/microsoft/oxidizer/blob/main/.github/prompts/bump-crate-version.prompt.md
    $dependents = @(Get-AllTransitiveDependents -crateName $CrateName -repoRoot $repoRoot)
    if ($dependents.Count -gt 0) {
        Write-Host ""
        Write-Host "🔗 Cascading $cascadeBump bump to $($dependents.Count) dependent crate(s): $($dependents -join ', ')" -ForegroundColor Cyan

        $cascadeReason = @{
            Target   = $CrateName
            Version  = $newVersion
            Breaking = ($cascadeBump -eq 'major')
        }

        foreach ($dependent in $dependents) {
            $depFolder = Join-Path $repoRoot 'crates' $dependent
            $depCargo  = Join-Path $depFolder 'Cargo.toml'
            $depChange = Join-Path $depFolder 'CHANGELOG.md'

            if (-not (Test-Path $depCargo)) {
                Write-Warning "Skipping cascade for '$dependent': Cargo.toml not found at '$depCargo'."
                continue
            }

            $depOld = Get-CurrentVersion -cargoTomlPath $depCargo
            $depNew = Invoke-CrateRelease -crateName $dependent -crateFolder $depFolder `
                -crateCargoToml $depCargo -rootCargoToml $rootCargoToml -changelogFile $depChange `
                -prBaseUrl $prBaseUrl -version "" -bump $cascadeBump -cascadeReason $cascadeReason

            $releases += [pscustomobject]@{ Crate = $dependent; OldVersion = $depOld; NewVersion = $depNew }
        }
    }

    # One workspace-wide cargo check after every version edit to confirm the workspace still resolves.
    Write-Host ""
    Write-Host "🔍 Running workspace cargo check..." -ForegroundColor Cyan
    cargo check --workspace --quiet | Write-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Workspace 'cargo check' failed after version updates. Please verify the changes." -ErrorAction Stop
    }

    Show-ReleaseSummary -releases $releases
    Show-FinalMessage -crateName $CrateName -newVersion $newVersion
}
catch {
    Write-Error "Script failed: $_"
    Exit 1
}
