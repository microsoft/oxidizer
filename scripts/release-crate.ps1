# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates the version of a Rust crate and generates a CHANGELOG.md file based on git history.

.DESCRIPTION
    This script automates two main tasks for releasing a Rust crate in a workspace repository:
    1. Version Bump: Automatically increment the version (major, minor, or patch) or set a specific version.
    2. Changelog Generation: It generates a CHANGELOG.md file based on git commit history.

    By default, if neither --version nor --bump is specified, the script will bump the minor version
    and reset the patch version to 0 (e.g., 1.2.3 -> 1.3.0).

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
$script:TypeOrder = @('feat', 'fix', 'perf', 'docs', 'task', 'refactor', 'build', 'ci', 'style')

# Defines commit types that should be excluded from the changelog.
$script:IgnoredTypes = @('test')

# --- COMPILED REGEX PATTERNS ---

# Pattern for conventional commit format: type(scope): description
$script:ConventionalCommitRegex = [regex]'^(\w+)(?:\(.*\))?:\s*(.*)'

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
        if ($conventionalMatch.Success) {
            $type = $conventionalMatch.Groups[1].Value
            $description = $conventionalMatch.Groups[2].Value
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

        $groupKey = if ($script:TypeGroupMapping.ContainsKey($type)) { $script:TypeGroupMapping[$type] } else { $type }

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
            $formattedLines += "- $headerName", "", $groupedCommits[$type], ""
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
        $versionParts = $currentVersion.Split('.')
        # Ensure versionParts has 3 elements (major, minor, patch)
        while ($versionParts.Count -lt 3) {
            $versionParts += '0'
        }

        # Determine which version component to bump
        $bumpType = if ([string]::IsNullOrEmpty($bump)) { 'minor' } else { $bump }

        switch ($bumpType) {
            'major' {
                $versionParts[0] = [int]$versionParts[0] + 1
                $versionParts[1] = '0'
                $versionParts[2] = '0'
            }
            'minor' {
                $versionParts[1] = [int]$versionParts[1] + 1
                $versionParts[2] = '0'
            }
            'patch' {
                $versionParts[2] = [int]$versionParts[2] + 1
            }
        }

        $newVersion = $versionParts -join '.'
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
        [string]$prBaseUrl
    )

    $tags = Invoke-GitCommand -Command "tag --list `"$crateName-v*`"" -ErrorMessage "Failed to retrieve git tags"
    if ($null -eq $tags -or $tags.Count -eq 0) {
        Write-Warning "No tags found for crate '$crateName'. Generating changelog from all history."
        $tags = @()
    } else {
        $filteredTags = @($tags | Where-Object { $_ -match "^${crateName}-v\d+\.\d+\.\d+$" })
        if ($filteredTags.Count -gt 0) {
            $tags = @($filteredTags | Sort-Object { [version]($_ -replace "${crateName}-v", '') })
        } else {
            Write-Warning "No valid semantic version tags found for crate '$crateName'. Generating changelog from all history."
            $tags = @()
        }
    }

    $changelogContent = @("# Changelog", "")
    $currentDate = (Get-Date).ToString('yyyy-MM-dd')
    $hasContent = $false

    # Process unreleased commits
    $latestTag = if ($tags.Count -gt 0) { $tags[-1] } else { $null }
    $range = if ($latestTag) { "$latestTag..HEAD" } else { "HEAD" }
    $rawCommits = Invoke-GitCommand -Command "log $range --pretty=format:`"%s`" -- `"$crateFolder`"" -ErrorMessage "Failed to retrieve git log for unreleased commits"
    if ($null -eq $rawCommits -or $rawCommits.Count -eq 0) {
        $rawCommits = @()
    } else {
        $rawCommits = @($rawCommits)
    }

    if ($rawCommits) {
        $formattedCommits = Format-ConventionalCommits -rawCommitMessages $rawCommits -prBaseUrl $prBaseUrl
        if ($formattedCommits) {
            $changelogContent += "## [$newVersion] - $currentDate", ""
            $changelogContent += $formattedCommits
            $changelogContent += ""
            $hasContent = $true
        }
    }

    # Process commits from previous tags
    for ($i = $tags.Count - 1; $i -ge 0; $i--) {
        $currentTag = $tags[$i]
        $previousTag = if ($i -gt 0) { $tags[$i-1] } else { $null }
        $range = if ($previousTag) { "$previousTag..$currentTag" } else { $currentTag }
        $rawCommits = Invoke-GitCommand -Command "log $range --pretty=format:`"%s`" -- `"$crateFolder`"" -ErrorMessage "Failed to retrieve git log for tag $currentTag"
        if ($null -eq $rawCommits -or $rawCommits.Count -eq 0) {
            $rawCommits = @()
        } else {
            $rawCommits = @($rawCommits)
        }
        if ($rawCommits.Count -eq 0) {
            continue
        }

        if ($rawCommits) {
            $formattedCommits = Format-ConventionalCommits -rawCommitMessages $rawCommits -prBaseUrl $prBaseUrl
            if ($formattedCommits) {
                $currentTagVersion = $currentTag -replace "${crateName}-v", ''
                $tagDateResult = Invoke-GitCommand -Command "log -1 --format=%ai `"$currentTag`"" -ErrorMessage "Failed to get date for tag $currentTag"
                $tagDate = if ($tagDateResult) { $tagDateResult.Split(' ')[0] } else { "unknown" }
                $changelogContent += "## [$currentTagVersion] - $tagDate", ""
                $changelogContent += $formattedCommits
                $changelogContent += ""
                $hasContent = $true
            }
        }
    }

    if (-not $hasContent) {
        Write-Warning "No relevant commits found to generate a changelog."
        return
    }

    $changelogContent | Out-File -FilePath $changelogFile -Encoding utf8
    Write-Host "✅ Changelog written to '$changelogFile'."
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

$crateFolder = Join-Path $repoRoot "crates/$CrateName"
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
    $newVersion = Update-CrateVersion -crateName $CrateName -version $Version -bump $Bump -crateCargoToml $crateCargoToml -rootCargoToml $rootCargoToml
    if ($null -eq $newVersion) {
        Write-Error "Failed to update crate version. Aborting."
        Exit 1
    }

    Write-Changelog -crateName $CrateName -newVersion $newVersion -crateFolder $crateFolder -changelogFile $changelogFile -prBaseUrl $prBaseUrl
    Show-FinalMessage -crateName $CrateName -newVersion $newVersion
}
catch {
    Write-Error "Script failed: $_"
    Exit 1
}
