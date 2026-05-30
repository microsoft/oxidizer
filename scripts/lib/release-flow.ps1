# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Release-flow library: helpers and orchestration for scripts/release-crate.ps1.

.DESCRIPTION
    Owns the orchestration helpers, changelog formatters, and the Invoke-ReleaseMain
    entrypoint that drives the full package-release workflow. scripts/release-crate.ps1
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
#   - SemVer arithmetic (Compare-SemanticVersions, Get-NextVersion, Get-ChangeTypeFromVersions,
#     Test-IsBreakingChange) and package-version readers (Get-CurrentVersion,
#     Get-PackageVersionFromRef).
#   - Workspace metadata (Get-WorkspaceMetadata, Get-WorkspacePackages,
#     Invalidate-WorkspaceMetadataCache, Test-PackageExposesTarget, Get-AllTransitiveDependents).
#   - Modified-but-unreleased dependency analysis (Get-PackagesWithUnreleasedChanges,
#     Get-PackagesWithVersionChanges, Get-UnreleasedModifiedDependencies).
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

# Parses the -Packages argument array of `release-packages.ps1` into structured
# release request entries. Each token is a string of the form '<name>@<change>',
# where <change> is one of:
#
#   - breaking, nonbreaking, patch  (case-insensitive change-type keywords)
#   - 1.0.0                         (the one-time 0.x → 1.0.0 graduation token)
#   - x.y.z                         (explicit semver pin — exactly three dotted
#                                    non-negative integers)
#
# Returns an array of pscustomobject entries, one per token:
#
#   @{
#     Name                   = '<as-typed>'             # case preserved
#     RequestedChangeType    = 'breaking'|'non-breaking'|'patch'|$null
#     RequestedTargetVersion = '1.2.3'|$null
#     IsGraduation           = $true|$false             # exactly '1.0.0' token
#     RawToken               = '<original token>'
#   }
#
# For the graduation keyword '1.0.0', the entry has both fields populated:
# RequestedChangeType='breaking', RequestedTargetVersion='1.0.0',
# IsGraduation=$true. The resolver later treats it as a breaking change with a
# pinned target version, and errors if the package is already at >=1.0.0.
#
# Validation:
#   - The -Packages array must contain at least one token.
#   - Each token must contain exactly one '@', neither at the start nor at the
#     end. Names are validated via Test-ValidPackageName. Duplicate names
#     (case-insensitive) are rejected so the resolver receives a clean unique
#     keyset.
#   - Whitespace-only tokens are rejected; leading/trailing whitespace around
#     a token is trimmed first.
#   - Unknown change-type keywords or malformed semvers throw a descriptive
#     error that quotes both the token and the offending change-spec text.
function Parse-ReleaseTokens {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [AllowEmptyString()]
        [AllowNull()]
        [string[]]$Tokens
    )

    if ($null -eq $Tokens -or @($Tokens).Count -eq 0) {
        throw "No packages to release. Provide at least one '<name>@<change>' token via -Packages."
    }

    $seenNames = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $results = New-Object 'System.Collections.Generic.List[object]'

    foreach ($raw in $Tokens) {
        if ($null -eq $raw) {
            throw "Encountered a null token in -Packages. Each entry must be a '<name>@<change>' string."
        }
        $token = $raw.Trim()
        if ([string]::IsNullOrEmpty($token)) {
            throw "Encountered an empty or whitespace-only token in -Packages. Each entry must be a '<name>@<change>' string."
        }

        $firstAt = $token.IndexOf('@')
        $lastAt  = $token.LastIndexOf('@')
        if ($firstAt -lt 1 -or $firstAt -ge ($token.Length - 1) -or $firstAt -ne $lastAt) {
            throw "Malformed package token '$raw'. Expected the form '<name>@<change>' with exactly one '@' separating a non-empty package name from a non-empty change specifier (e.g. 'bytesbuf@breaking', 'fetch_hyper@1.2.3', 'http_extensions@1.0.0')."
        }

        $name       = $token.Substring(0, $firstAt)
        $changeSpec = $token.Substring($firstAt + 1)

        if (-not (Test-ValidPackageName -packageName $name)) {
            throw "Invalid package name '$name' in token '$raw'. Package names must contain only letters, numbers, hyphens, and underscores; must not start or end with a hyphen; and must be 64 characters or less."
        }

        if (-not $seenNames.Add($name)) {
            throw "Duplicate package name '$name' in -Packages list. Each package may appear at most once; release each package with a single combined change type."
        }

        $requestedChangeType    = $null
        $requestedTargetVersion = $null
        $isGraduation           = $false

        switch -CaseSensitive ($changeSpec.ToLowerInvariant()) {
            'breaking'    { $requestedChangeType = 'breaking';     break }
            'nonbreaking' { $requestedChangeType = 'non-breaking'; break }
            'patch'       { $requestedChangeType = 'patch';        break }
            default {
                if ($changeSpec -match '^\d+\.\d+\.\d+$') {
                    if ($changeSpec -eq '1.0.0') {
                        $isGraduation           = $true
                        $requestedChangeType    = 'breaking'
                        $requestedTargetVersion = '1.0.0'
                    } else {
                        $requestedTargetVersion = $changeSpec
                    }
                } else {
                    throw "Invalid change specifier '$changeSpec' in token '$raw'. Expected one of: 'breaking', 'nonbreaking', 'patch', '1.0.0' (one-time 0.x→1.0.0 graduation), or an explicit semantic version 'x.y.z'."
                }
            }
        }

        $results.Add([pscustomobject]@{
            Name                   = $name
            RequestedChangeType    = $requestedChangeType
            RequestedTargetVersion = $requestedTargetVersion
            IsGraduation           = $isGraduation
            RawToken               = $raw
        })
    }

    return $results.ToArray()
}

# BFS over a workspace baseline to find all published transitive dependents of
# a cargo package (identified by its underscore-normalized cargo name). Mirrors
# Get-AllTransitiveDependents but operates against an in-memory baseline
# snapshot, so it produces deterministic answers even after disk state changes.
#
# Behaviour parity: traverses through unpublished workspace packages (so they
# act as conduits between published packages) but only returns published
# packages in the result list. The target itself is never returned.
function Get-TransitivePublishedDependentsFromBaseline {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$Baseline,
        [Parameter(Mandatory = $true)][string]$TargetCargoName
    )

    $toVisit = [System.Collections.Generic.Queue[string]]::new()
    $toVisit.Enqueue($TargetCargoName)
    $visited = [System.Collections.Generic.HashSet[string]]::new()
    [void]$visited.Add($TargetCargoName)

    $dependents = New-Object 'System.Collections.Generic.List[string]'
    while ($toVisit.Count -gt 0) {
        $current = $toVisit.Dequeue()
        foreach ($candidate in $Baseline) {
            $candidateNorm = $candidate.Name.Replace('-', '_')
            if ($visited.Contains($candidateNorm)) { continue }
            if ($candidate.Deps -contains $current) {
                [void]$visited.Add($candidateNorm)
                $toVisit.Enqueue($candidateNorm)
                if ($candidate.Published) {
                    $dependents.Add($candidate.Folder)
                }
            }
        }
    }

    return @($dependents)
}

# Turns the parsed token entries from Parse-ReleaseTokens into a *resolved
# release set* — every package that will receive a release in this invocation,
# whether the user asked for it directly or it was pulled in by cascade.
#
# Inputs:
#   -ParsedTokens     : the @() output of Parse-ReleaseTokens.
#   -WorkspaceBaseline: an *immutable* snapshot of Get-WorkspacePackages,
#                       captured BEFORE any release writes are performed. The
#                       same snapshot must be passed to every Resolve-ReleaseSet
#                       call during a single release-packages run, otherwise
#                       cascade math would double-bump (the on-disk state
#                       mutates as releases land).
#
# Returns: an array of pscustomobject entries, one per resolved package:
#
#   @{
#     Folder                  = '<crate folder>'
#     Name                    = '<cargo package name>'  # may differ from Folder
#     CurrentVersion          = '<baseline version>'
#     RequestedChangeType     = 'breaking'|'non-breaking'|'patch'|$null   # null for cascade-source
#     RequestedTargetVersion  = '<pin>'|$null                              # null when not pinned
#     IsGraduation            = $true|$false
#     EffectiveChangeType     = 'breaking'|'non-breaking'|'patch'         # after cascade resolution
#     EffectiveTargetVersion  = '<version>'                                # after cascade resolution
#     Source                  = 'user'|'cascade'
#     AutoUpgraded            = $true|$false   # user-source entry strengthened by cascade
#     CascadeReasons          = [List<{Target,Version,Breaking}>]          # one per (target → dep) edge
#     RawToken                = '<original token>'|$null                   # null for cascade-source
#   }
#
# Resolution algorithm:
#   1. Seed every token as a user-source entry. Reject:
#        - tokens for non-workspace packages
#        - tokens for unpublished workspace packages
#        - explicit version pins not strictly greater than the current version
#        - graduation '1.0.0' applied to a package already at >= 1.0.0
#   2. For each user-source entry, BFS via
#      Get-TransitivePublishedDependentsFromBaseline to collect all published
#      transitive dependents. For each dependent compute the cascade-applied
#      change type from exposing/non-exposing semantics (mirrors the existing
#      Invoke-ReleaseFlow cascade), and either:
#        - upgrade an existing entry (rank-ordered: patch < non-breaking <
#          breaking). For user-source entries with -Change keyword,
#          auto-upgrade silently and set AutoUpgraded=$true. For user-source
#          entries with an explicit version pin, throw if the pin would
#          numerically undershoot the cascade-required version; otherwise
#          honour the pin and bump only the change-type tag.
#        - or create a new cascade-source entry.
#      Cascade reasons are recorded per (target → dep) edge with dedup by
#      target name (re-encountering an edge for an already-strengthened target
#      overwrites the prior reason in place).
#
# Note: cascade is one-level. The set of dependents reachable from a user
# target is the transitive published dependents BFS, but the cascade-applied
# change type for each dependent is derived from exposure of the USER TARGET
# (not of any intermediate). This matches Invoke-ReleaseFlow's pre-existing
# semantics; tightening the analysis is out of scope for the redesign.
function Resolve-ReleaseSet {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$ParsedTokens,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$WorkspaceBaseline
    )

    if ($null -eq $ParsedTokens -or @($ParsedTokens).Count -eq 0) {
        throw "Resolve-ReleaseSet: ParsedTokens is empty. Parse-ReleaseTokens should reject empty input upstream."
    }

    $baselineByFolder = @{}
    $baselineByCargo  = @{}
    foreach ($pkg in $WorkspaceBaseline) {
        $baselineByFolder[$pkg.Folder] = $pkg
        $baselineByCargo[$pkg.Name.Replace('-', '_')] = $pkg
    }

    $rank = @{ 'patch' = 1; 'non-breaking' = 2; 'breaking' = 3 }

    $resolved = [ordered]@{}

    foreach ($req in $ParsedTokens) {
        $pkg = $baselineByFolder[$req.Name]
        if ($null -eq $pkg) {
            $normalizedReq = $req.Name.Replace('-', '_')
            $pkg = $baselineByCargo[$normalizedReq]
        }
        if ($null -eq $pkg) {
            throw "Package '$($req.Name)' is not part of the workspace (no folder under 'crates/' and no Cargo package by that name). Token: '$($req.RawToken)'."
        }
        if (-not $pkg.Published) {
            throw "Package '$($pkg.Folder)' has 'publish = false' in its Cargo.toml; only published packages can be released. Token: '$($req.RawToken)'."
        }

        if ($resolved.Contains($pkg.Folder)) {
            throw "Internal error: package '$($pkg.Folder)' resolved twice from -Packages (token '$($req.RawToken)'). Parse-ReleaseTokens should have rejected the duplicate upstream."
        }

        $currentVersion = $pkg.Version

        if (-not [string]::IsNullOrEmpty($req.RequestedTargetVersion)) {
            $target = $req.RequestedTargetVersion
            $cmp = Compare-SemanticVersions -version1 $target -version2 $currentVersion
            if ($cmp -le 0) {
                throw "Cannot release '$($pkg.Folder)' as v$($target): package is already at v$currentVersion. Explicit version pins must be strictly greater than the current version. Token: '$($req.RawToken)'."
            }
            if ($req.IsGraduation) {
                $currentMajor = [int]($currentVersion.Split('.')[0])
                if ($currentMajor -ge 1) {
                    throw "Cannot graduate '$($pkg.Folder)' to 1.0.0: package is already at v$currentVersion (>= 1.0.0). The '1.0.0' graduation token is only valid for packages whose current version is in the 0.x.y range. Token: '$($req.RawToken)'."
                }
            }
            $effectiveChangeType    = Get-ChangeTypeFromVersions -oldVersion $currentVersion -newVersion $target
            $effectiveTargetVersion = $target
        } else {
            $effectiveChangeType    = $req.RequestedChangeType
            $effectiveTargetVersion = Get-NextVersion -currentVersion $currentVersion -ChangeType $effectiveChangeType
        }

        $resolved[$pkg.Folder] = [pscustomobject]@{
            Folder                  = $pkg.Folder
            Name                    = $pkg.Name
            CurrentVersion          = $currentVersion
            RequestedChangeType     = $req.RequestedChangeType
            RequestedTargetVersion  = $req.RequestedTargetVersion
            IsGraduation            = $req.IsGraduation
            EffectiveChangeType     = $effectiveChangeType
            EffectiveTargetVersion  = $effectiveTargetVersion
            Source                  = 'user'
            AutoUpgraded            = $false
            CascadeReasons          = New-Object 'System.Collections.Generic.List[object]'
            RawToken                = $req.RawToken
        }
    }

    # Snapshot the user-source folder names before cascade adds cascade-source
    # entries — we don't iterate cascade-source entries for their own cascades
    # (Invoke-ReleaseFlow's pre-existing one-level semantics).
    $userFolders = @($resolved.Keys) | ForEach-Object { $_ }

    foreach ($targetFolder in $userFolders) {
        $targetEntry = $resolved[$targetFolder]
        $targetPkg   = $baselineByFolder[$targetFolder]

        $targetIsBreaking = Test-IsBreakingChange -oldVersion $targetEntry.CurrentVersion -ChangeType $targetEntry.EffectiveChangeType
        $exposingCascadeChangeType = if ($targetIsBreaking) { 'breaking' } else { $targetEntry.EffectiveChangeType }

        $targetCargoNorm = $targetPkg.Name.Replace('-', '_')
        $reachable = Get-TransitivePublishedDependentsFromBaseline -Baseline $WorkspaceBaseline -TargetCargoName $targetCargoNorm

        foreach ($depFolder in $reachable) {
            $depPkg  = $baselineByFolder[$depFolder]
            $exposes = Test-PackageExposesTarget -dependent $depPkg -targetPackageName $targetPkg.Name
            $dependentChangeType = if ($exposes) { $exposingCascadeChangeType } else { 'patch' }

            $depBreakingForReason = Test-IsBreakingChange -oldVersion $depPkg.Version -ChangeType $dependentChangeType
            $cascadeReason = [pscustomobject]@{
                Target   = $targetPkg.Name
                Version  = $targetEntry.EffectiveTargetVersion
                Breaking = $depBreakingForReason
            }

            if ($resolved.Contains($depFolder)) {
                $existing = $resolved[$depFolder]

                # Dedup cascade reasons by target name (re-encountering the
                # same edge after a strengthening pass overwrites the prior
                # reason in place rather than adding a duplicate).
                $existingReasonIdx = -1
                for ($i = 0; $i -lt $existing.CascadeReasons.Count; $i++) {
                    if ($existing.CascadeReasons[$i].Target -eq $cascadeReason.Target) {
                        $existingReasonIdx = $i
                        break
                    }
                }
                if ($existingReasonIdx -ge 0) {
                    $existing.CascadeReasons[$existingReasonIdx] = $cascadeReason
                } else {
                    $existing.CascadeReasons.Add($cascadeReason)
                }

                $existingRank = $rank[$existing.EffectiveChangeType]
                $newRank      = $rank[$dependentChangeType]
                if ($newRank -gt $existingRank) {
                    $cascadeRequiredVersion = Get-NextVersion -currentVersion $existing.CurrentVersion -ChangeType $dependentChangeType

                    if (-not [string]::IsNullOrEmpty($existing.RequestedTargetVersion)) {
                        # User pinned an explicit version. Verify it numerically
                        # satisfies the cascade requirement; if not, the user
                        # has to revise their request.
                        $cmpPin = Compare-SemanticVersions -version1 $existing.RequestedTargetVersion -version2 $cascadeRequiredVersion
                        if ($cmpPin -lt 0) {
                            $reasonsNames = ($existing.CascadeReasons | ForEach-Object { $_.Target } | Sort-Object -Unique) -join ', '
                            throw "Cannot release '$($existing.Folder)' as v$($existing.RequestedTargetVersion): cascade requires at least v$cascadeRequiredVersion because of changes in: $reasonsNames. Specify a higher version pin or use a change-type keyword."
                        }
                        # Pin still satisfies. Bump the EffectiveChangeType tag
                        # (so cascade re-exposure decisions for this entry's
                        # own dependents — if we iterated them, which we don't
                        # at present — would be correct) but keep the pin as
                        # the version.
                        $existing.EffectiveChangeType = $dependentChangeType
                    } else {
                        $existing.EffectiveChangeType    = $dependentChangeType
                        $existing.EffectiveTargetVersion = $cascadeRequiredVersion
                        if ($existing.Source -eq 'user') {
                            $existing.AutoUpgraded = $true
                        }
                    }
                }
            } else {
                $newEntry = [pscustomobject]@{
                    Folder                  = $depPkg.Folder
                    Name                    = $depPkg.Name
                    CurrentVersion          = $depPkg.Version
                    RequestedChangeType     = $null
                    RequestedTargetVersion  = $null
                    IsGraduation            = $false
                    EffectiveChangeType     = $dependentChangeType
                    EffectiveTargetVersion  = Get-NextVersion -currentVersion $depPkg.Version -ChangeType $dependentChangeType
                    Source                  = 'cascade'
                    AutoUpgraded            = $false
                    CascadeReasons          = New-Object 'System.Collections.Generic.List[object]'
                    RawToken                = $null
                }
                $newEntry.CascadeReasons.Add($cascadeReason)
                $resolved[$depPkg.Folder] = $newEntry
            }
        }
    }

    return @($resolved.Values)
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

function Update-PackageVersion {
    param(
        [string]$packageName,
        [string]$version,
        [string]$ChangeType,
        [string]$packageCargoToml,
        [string]$rootCargoToml
    )

    $currentVersion = Get-CurrentVersion -cargoTomlPath $packageCargoToml

    $newVersion = ""
    if ([string]::IsNullOrEmpty($version)) {
        $effectiveChangeType = if ([string]::IsNullOrEmpty($ChangeType)) { 'non-breaking' } else { $ChangeType }
        $newVersion = Get-NextVersion -currentVersion $currentVersion -ChangeType $effectiveChangeType
        # User-visible output uses CHANGE-TYPE vocabulary (breaking change /
        # non-breaking change / patch) — see AGENTS.md "Release Versioning
        # Vocabulary".
        $changeLabel = Get-ChangeTypeLabel -ChangeType $effectiveChangeType
        Write-Host "✅ Releasing $changeLabel`: $currentVersion -> $newVersion."
    }
    else {
        $newVersion = $version
        Write-Host "✅ Using specified version: $newVersion."
    }

    Write-Host "📝 Updating '$packageCargoToml'..."
    $packageContent = Get-Content $packageCargoToml -Raw
    # Scope the version replacement to the [package] table via the shared regex
    # in releasing.ps1, which anchors to line starts so substring keys like
    # `rust-version` cannot match and inline workspace-dep `version = "..."`
    # declarations later in the file are left alone. Replace exactly once.
    if (-not $script:CargoPackageVersionRegex.IsMatch($packageContent)) {
        Write-Error "Could not find [package] version line in '$packageCargoToml'." -ErrorAction Stop
    }
    $packageContent = $script:CargoPackageVersionRegex.Replace($packageContent, ('${1}' + $newVersion), 1)
    Set-Content $packageCargoToml -Value $packageContent -NoNewline

    Write-Host "📝 Updating '$rootCargoToml'..."

    function Get-EscapedRegexSpecialChars($str) {
        # Escape all regex metacharacters: . $ ^ { [ ( | ) * + ? \ /
        # The replacement string `\$1` produces a literal backslash followed by
        # the matched metacharacter — `\` is a literal in .NET replacement-string
        # syntax (not an escape) and `$1` is the group-1 backreference. Do NOT
        # use `\\$1` here: that double-escapes (e.g. `1.2.3` -> `1\\.2\\.3`).
        return ($str -replace $script:RegexEscapeRegex, '\$1')
    }

    $escapedPackageName = Get-EscapedRegexSpecialChars($packageName)
    $packageNamePattern = $escapedPackageName.Replace('_', '[-_]')
    $regex = '(?<=' + $packageNamePattern + '\s*=\s*\{[^\}]*?version\s*=\s*")[^"]+'
    (Get-Content $rootCargoToml -Raw) -replace $regex, $newVersion | Set-Content $rootCargoToml -NoNewline

    return $newVersion
}

# Re-stamps an already-pending release with a different version: the package's
# Cargo.toml, the workspace Cargo.toml's [workspace.dependencies] entry, and
# the CHANGELOG.md "## [oldVersion] - YYYY-MM-DD" section header are rewritten
# in place, preserving the existing changelog body (which was generated from
# the same commit/cascade history the user already reviewed).
#
# Used for in-place escalation when a cascade toward dependents or a subsequent
# Invoke-ReleaseFlow re-invocation needs to lift an already-incremented package
# to a higher version. The alternative — calling Invoke-PackageRelease again —
# would create a second `## [<new>]` changelog section while leaving the stale
# `## [<old>]` section in place, breaking the convention of one section per
# released version.
#
# Returns the new version string. No cargo metadata invalidation here — caller
# is responsible for calling Reset-ReleaseScriptCaches / Invalidate-WorkspaceMetadataCache
# (the wider cascade plumbing already does so where it matters).
function Update-PendingReleaseVersion {
    param(
        [Parameter(Mandatory = $true)][string]$PackageName,
        [Parameter(Mandatory = $true)][string]$PackageFolder,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $true)][string]$OldVersion,
        [Parameter(Mandatory = $true)][string]$NewVersion
    )

    if ($OldVersion -eq $NewVersion) { return $NewVersion }

    $packageCargoToml = Join-Path $PackageFolder 'Cargo.toml'
    $changelogFile  = Join-Path $PackageFolder 'CHANGELOG.md'

    if (-not (Test-Path -LiteralPath $packageCargoToml)) {
        Write-Error "Update-PendingReleaseVersion: Cargo.toml not found at '$packageCargoToml'." -ErrorAction Stop
    }

    Write-Host "📝 Re-stamping '$packageCargoToml' from $OldVersion to $NewVersion..."
    $packageContent = Get-Content -LiteralPath $packageCargoToml -Raw
    if (-not $script:CargoPackageVersionRegex.IsMatch($packageContent)) {
        Write-Error "Update-PendingReleaseVersion: no [package] version line in '$packageCargoToml'." -ErrorAction Stop
    }
    # Replace exactly one occurrence (the [package].version), preserving the
    # captured prefix via $1 so the surrounding whitespace / `version = "` is
    # left intact.
    $packageContent = $script:CargoPackageVersionRegex.Replace($packageContent, ('${1}' + $NewVersion), 1)
    Set-Content -LiteralPath $packageCargoToml -Value $packageContent -NoNewline

    Write-Host "📝 Re-stamping '$RootCargoToml' workspace entry for '$PackageName' to $NewVersion..."
    function Get-EscapedRegexSpecialChars2($str) {
        return ($str -replace $script:RegexEscapeRegex, '\$1')
    }
    $escapedPackageName = Get-EscapedRegexSpecialChars2($PackageName)
    $packageNamePattern = $escapedPackageName.Replace('_', '[-_]')
    # Same lookbehind regex shape as Update-PackageVersion so the same
    # workspace-dep declaration pattern matches consistently.
    $regex = '(?<=' + $packageNamePattern + '\s*=\s*\{[^\}]*?version\s*=\s*")[^"]+'
    (Get-Content -LiteralPath $RootCargoToml -Raw) -replace $regex, $NewVersion | Set-Content -LiteralPath $RootCargoToml -NoNewline

    if (Test-Path -LiteralPath $changelogFile) {
        Write-Host "📝 Re-stamping '$changelogFile' section header from [$OldVersion] to [$NewVersion]..."
        $eol = Get-FileLineEnding -Path $changelogFile
        $existing = Get-Content -LiteralPath $changelogFile -Raw
        $escapedOld = $script:RegexEscapeRegex.Replace($OldVersion, '\$1')
        # Rewrite the FIRST occurrence of "## [oldVersion]" — section headers
        # are unique per version, so [regex]::new(...).Replace(input, repl, 1)
        # is sufficient and avoids accidentally clobbering body text that
        # mentions the version.
        $headerRegex = [regex]"## \[$escapedOld\]"
        if (-not $headerRegex.IsMatch($existing)) {
            Write-Warning "Update-PendingReleaseVersion: no '## [$OldVersion]' section in '$changelogFile'; leaving changelog alone."
        } else {
            $rewritten = $headerRegex.Replace($existing, "## [$NewVersion]", 1)
            # Re-normalize line endings: the regex replace doesn't change line
            # boundaries, but Set-Content with -NoNewline preserves whatever
            # mix the file already had. Force the file's original convention.
            $hadTrailingNewline = $existing.EndsWith("`n") -or $existing.EndsWith("`r`n")
            if ($hadTrailingNewline -and -not ($rewritten.EndsWith("`n") -or $rewritten.EndsWith("`r`n"))) {
                $rewritten += $eol
            }
            Set-Content -LiteralPath $changelogFile -Value $rewritten -NoNewline -Encoding utf8
        }
    } else {
        Write-Warning "Update-PendingReleaseVersion: changelog '$changelogFile' not found; skipping header rewrite."
    }

    return $NewVersion
}

function Write-Changelog {
    param(
        [string]$packageName,
        [string]$newVersion,
        [string]$packageFolder,
        [string]$changelogFile,
        [string]$prBaseUrl,
        # Optional: when this package is being released as a cascade-from-dependency,
        # describe one or more cascades so a maintenance/breaking entry can be
        # written even if the package has no commits since its last release. Each
        # element shape: @{ Target = '<name>'; Version = '<x.y.z>'; Breaking = $false }.
        # The section header is `⚠️ Breaking` if ANY reason is Breaking, otherwise
        # `🔧 Maintenance`; one bullet is emitted per reason in deterministic
        # (Target-sorted) order. Element shape is duck-typed (.Target / .Version /
        # .Breaking) so both hashtables and [pscustomobject] are accepted; the
        # bundled-input path (Resolve-ReleaseSet) builds pscustomobjects while the
        # legacy Invoke-CascadeStep path builds hashtables.
        [object[]]$cascadeReasons = $null
    )

    $hasCascade = ($null -ne $cascadeReasons) -and ($cascadeReasons.Count -gt 0)

    $tags = Invoke-Git -Arguments @('tag', '--list', "$packageName-v*")
    $latestTag = $null
    if ($null -eq $tags -or $tags.Count -eq 0) {
        Write-Warning "No tags found for package '$packageName'. Generating changelog from all history."
    } else {
        $filteredTags = @($tags | Where-Object { $_ -match "^${packageName}-v\d+\.\d+\.\d+$" })
        if ($filteredTags.Count -gt 0) {
            $sortedTags = @($filteredTags | Sort-Object { [version]($_ -replace "${packageName}-v", '') })
            $latestTag = $sortedTags[-1]
        } else {
            Write-Warning "No valid semantic version tags found for package '$packageName'. Generating changelog from all history."
        }
    }

    $currentDate = (Get-Date).ToString('yyyy-MM-dd')

    # Get commits since the latest tag (unreleased commits)
    $range = if ($latestTag) { "$latestTag..HEAD" } else { "HEAD" }
    $rawCommits = Invoke-Git -Arguments @('log', $range, '--pretty=format:%s', '--', $packageFolder)
    if ($null -eq $rawCommits -or $rawCommits.Count -eq 0) {
        $rawCommits = @()
    } else {
        $rawCommits = @($rawCommits)
    }

    $formattedCommits = @()
    if ($rawCommits.Count -gt 0) {
        $formattedCommits = Format-ConventionalCommits -rawCommitMessages $rawCommits -prBaseUrl $prBaseUrl
    }

    if ($formattedCommits.Count -eq 0 -and -not $hasCascade) {
        if ($rawCommits.Count -eq 0) {
            Write-Warning "No unreleased commits found to add to the changelog."
        } else {
            $filteredCount = $rawCommits.Count
            $noun = if ($filteredCount -eq 1) { 'commit was' } else { 'commits were' }
            Write-Warning "No relevant commits found to add to the changelog (all $filteredCount $noun filtered out)."
        }
        return
    }

    # Prepend cascade entries when this package is being released because one
    # (or more) of its dependencies was released. Emits structured
    # "Now requires <version> of <target>" bullets — deliberately formal
    # rather than colloquial — under the appropriate section:
    #   - 🔧 Maintenance        (when no contributing cascade is breaking)
    #   - ⚠️ Breaking           (when at least one contributing cascade is breaking)
    # Bullets are sorted by Target name for deterministic output across runs.
    # If the same section header was already produced by
    # Format-ConventionalCommits for this release, the cascade bullets are
    # merged into that existing section instead of creating a duplicate header.
    if ($hasCascade) {
        $anyBreaking = $false
        foreach ($r in $cascadeReasons) {
            if ([bool]$r.Breaking) { $anyBreaking = $true; break }
        }
        $sectionHeader = if ($anyBreaking) { '- ⚠️ Breaking' } else { '- 🔧 Maintenance' }

        $sortedReasons = @($cascadeReasons | Sort-Object -Property @{ Expression = { $_.Target } })
        $cascadeBullets = @($sortedReasons | ForEach-Object {
            "  - Now requires ``$($_.Version)`` of ``$($_.Target)``"
        })

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
            $formattedCommits = $before + $cascadeBullets + $after
        } else {
            $cascadeLines = @($sectionHeader, "") + $cascadeBullets
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
        [string]$packageName,
        [string]$packageFolder
    )

    $readmeTemplate = Join-Path $packageFolder "../README.j2"
    if (-not (Test-Path $readmeTemplate)) {
        Write-Warning "README template not found at '$readmeTemplate'. Skipping README generation."
        return
    }

    if (-not (Test-CommandExists -command "cargo-doc2readme")) {
        Write-Warning "cargo-doc2readme is not installed. Skipping README generation. Install with: cargo install cargo-doc2readme"
        return
    }

    Write-Host "📝 Updating README.md..."
    Push-Location $packageFolder
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

# Increments a single package's version, regenerates its changelog and README.
# Returns the new version string.
function Invoke-PackageRelease {
    param(
        [string]$packageName,
        [string]$packageFolder,
        [string]$packageCargoToml,
        [string]$rootCargoToml,
        [string]$changelogFile,
        [string]$prBaseUrl,
        [string]$version,
        [string]$ChangeType,
        [object[]]$cascadeReasons = $null
    )

    $newVersion = Update-PackageVersion -packageName $packageName -version $version -ChangeType $ChangeType `
        -packageCargoToml $packageCargoToml -rootCargoToml $rootCargoToml
    if ($null -eq $newVersion) {
        Write-Error "Failed to update version for package '$packageName'." -ErrorAction Stop
    }

    Write-Changelog -packageName $packageName -newVersion $newVersion -packageFolder $packageFolder `
        -changelogFile $changelogFile -prBaseUrl $prBaseUrl -cascadeReasons $cascadeReasons
    Update-Readme -packageName $packageName -packageFolder $packageFolder

    return $newVersion
}

function Show-ReleaseSummary {
    param(
        [array]$releases
    )

    Write-Host ""
    Write-Host "📦 Released packages:" -ForegroundColor Green
    foreach ($r in $releases) {
        Write-Host "  - $($r.Package): $($r.OldVersion) -> $($r.NewVersion)" -ForegroundColor Green
    }
    Write-Host ""
}

function Show-FinalMessage {
    param(
        [Parameter(Mandatory = $true)][string]$PackageName,
        [Parameter(Mandatory = $true)][array]$Releases
    )

    # Locate the primary release record (the package the user originally asked
    # for). Defensive fallback to the first release in case it's missing — this
    # shouldn't happen in practice but we never want the post-success message
    # to crash and stamp the run as failed.
    $primary = $Releases | Where-Object { $_.Package -eq $PackageName } | Select-Object -First 1
    if ($null -eq $primary) { $primary = $Releases | Select-Object -First 1 }
    $primaryName    = $primary.Package
    $primaryVersion = $primary.NewVersion

    $extraCount = @($Releases).Count - 1
    if ($extraCount -le 0) {
        # Single-package release: a scoped feat(<package>): prefix is the most
        # informative form because the commit really is about that one package.
        $commitMessage = "feat($primaryName): release v$primaryVersion"
    } else {
        # Multi-package release: the conventional-commits scope would be
        # misleading because the commit spans many packages. Drop the scope
        # and call out the extras so reviewers see at a glance that this is
        # a coordinated release, not a single-package release.
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
# already been version-incremented (sufficiently) in an earlier cascade pass
# within the same PR — we don't want to increment again, but we still want
# to record that this new release also pulled through. Operates by reading
# the file, locating the target section, finding (or creating) the
# appropriate `- 🔧 Maintenance` or `- ⚠️ Breaking` sub-header, and inserting
# the bullet unless an exact match already exists.
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
    # Capture trailing-newline status from the raw bytes — Get-Content above
    # invisibly strips the file's final terminator, so a $lines[-1] -eq ''
    # check would only fire when the file ends with TWO newlines (a blank
    # trailing line). The normal CHANGELOG.md shape — one trailing '\n' — would
    # otherwise be silently flattened on every cascade write.
    $rawForEol = [System.IO.File]::ReadAllText($ChangelogFile)
    $hadTrailingNewline = $rawForEol.EndsWith("`n") -or $rawForEol.EndsWith("`r`n")
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

    # Drop any pre-existing "Now requires <any-version> of <targetName>" bullets
    # within this section. Escalations re-fire cascades with a higher target
    # version (and possibly a different Breaking/Maintenance classification);
    # without this dedup, the section would accumulate stale bullets citing the
    # old target version. Matches `<TargetName>` exactly (regex-escaped) so a
    # bullet for an unrelated target with a similar name is left alone.
    $escapedTargetName = $script:RegexEscapeRegex.Replace($targetName, '\$1')
    $bulletForSameTarget = '^\s*-\s+Now requires `[^`]+` of `' + $escapedTargetName + '`\s*$'
    $cleaned = New-Object 'System.Collections.Generic.List[string]'
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($i -gt $sectionStart -and $i -lt $sectionEnd -and $lines[$i] -match $bulletForSameTarget) {
            continue
        }
        $cleaned.Add($lines[$i])
    }
    $linesRemoved = $lines.Count - $cleaned.Count
    if ($linesRemoved -gt 0) {
        # Re-locate section boundaries after the removal (section shrunk).
        $lines = $cleaned.ToArray()
        $sectionEnd -= $linesRemoved
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
    $body = ($new -join $eol)
    if ($hadTrailingNewline -and -not $body.EndsWith($eol)) { $body += $eol }
    Set-Content -LiteralPath $ChangelogFile -Value $body -NoNewline -Encoding utf8
    Write-Host "📝 Recorded cascade in '$ChangelogFile' under [$Version]." -ForegroundColor DarkCyan
}

# Cascades a single dependent. Idempotent on already-released packages: if the dependent
# has already had its version incremented during this PR (its on-disk version differs from
# $BaseRef) we either skip the re-application (when the existing version is sufficient) or
# upgrade to the required version.
function Invoke-CascadeStep {
    param(
        [Parameter(Mandatory = $true)][string]$Dependent,
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$PrBaseUrl,
        [Parameter(Mandatory = $true)][string]$TargetPackageName,
        [Parameter(Mandatory = $true)][string]$TargetNewVersion,
        [Parameter(Mandatory = $true)][string]$DependentChangeType,
        [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$BaseRef
    )

    $depFolder = Join-Path $RepoRoot 'crates' $Dependent
    $depCargo  = Join-Path $depFolder 'Cargo.toml'
    $depChange = Join-Path $depFolder 'CHANGELOG.md'

    if (-not (Test-Path $depCargo)) {
        Write-Warning "Skipping cascade for '$Dependent': Cargo.toml not found at '$depCargo'."
        return $null
    }

    $depCurrent = Get-CurrentVersion -cargoTomlPath $depCargo
    $depBase = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $Dependent

    $depCascadeReason = @{
        Target   = $TargetPackageName
        Version  = $TargetNewVersion
        Breaking = (Test-IsBreakingChange -oldVersion $depCurrent -ChangeType $DependentChangeType)
    }

    # New package (no base-ref Cargo.toml): behave as the legacy
    # cascade did — let Invoke-PackageRelease increment the version from $depCurrent.
    if ([string]::IsNullOrEmpty($depBase) -or $depCurrent -eq $depBase) {
        $depNew = Invoke-PackageRelease -packageName $Dependent -packageFolder $depFolder `
            -packageCargoToml $depCargo -rootCargoToml $RootCargoToml -changelogFile $depChange `
            -prBaseUrl $PrBaseUrl -version "" -ChangeType $DependentChangeType -cascadeReasons @($depCascadeReason)
        Invalidate-WorkspaceMetadataCache
        return [pscustomobject]@{ Package = $Dependent; OldVersion = $depCurrent; NewVersion = $depNew }
    }

    # Already version-incremented in this PR. Compute the version that THIS cascade would
    # have produced starting from the base-ref version, and compare.
    $required = Get-NextVersion -currentVersion $depBase -ChangeType $DependentChangeType
    $cmp = Compare-SemanticVersions -version1 $depCurrent -version2 $required

    if ($cmp -ge 0) {
        Write-Host "  • $Dependent already at $depCurrent (>= required $required); recording cascade only." -ForegroundColor DarkGray
        Add-CascadeBulletToVersionSection -ChangelogFile $depChange -Version $depCurrent -CascadeReason $depCascadeReason
        return [pscustomobject]@{ Package = $Dependent; OldVersion = $depCurrent; NewVersion = $depCurrent }
    }

    Write-Host "  • $Dependent currently $depCurrent < required $required; upgrading." -ForegroundColor DarkYellow
    # In-place re-stamp: the dependent already has a pending changelog section
    # at $depCurrent (commit bullets + any earlier cascade bullets). Calling
    # Invoke-PackageRelease here would generate a second `## [$required]` section
    # while leaving the stale `## [$depCurrent]` section behind. Update the
    # version in Cargo.toml + workspace Cargo.toml + the existing CHANGELOG
    # header, then append the new cascade bullet under the now-renamed section.
    Update-PendingReleaseVersion -PackageName $Dependent -PackageFolder $depFolder `
        -RootCargoToml $RootCargoToml -OldVersion $depCurrent -NewVersion $required | Out-Null
    Add-CascadeBulletToVersionSection -ChangelogFile $depChange -Version $required -CascadeReason $depCascadeReason
    Invalidate-WorkspaceMetadataCache
    return [pscustomobject]@{ Package = $Dependent; OldVersion = $depCurrent; NewVersion = $required }
}

# --- CASCADE-MESSAGE FORMATTING ---
#
# Pure helpers backing the cascade announcement printed by Invoke-ReleaseFlow.
# Split out so the human-facing wording (and the "downgrade by one level"
# mapping for non-exposing dependents) can be unit-tested without driving the
# full release flow.

# Maps a change type to the semantic label shown in the cascade announcement
# (full form, used as 'as <label>'): breaking → 'breaking change',
# non-breaking → 'non-breaking change', patch → 'patch'.
function Get-ChangeTypeLabel {
    param([Parameter(Mandatory = $true)][ValidateSet('breaking', 'non-breaking', 'patch')][string]$ChangeType)

    switch ($ChangeType) {
        'breaking'     { return 'breaking change' }
        'non-breaking' { return 'non-breaking change' }
        'patch'        { return 'patch' }
    }
}

# Short form of the semantic label, used inside the parenthetical that
# describes the downgrade for non-exposing dependents (e.g. "or non-breaking
# if no API exposure of '<target>'"). Mirrors Get-ChangeTypeLabel without the
# trailing 'change' noun where it would read awkwardly.
function Get-ShortChangeTypeLabel {
    param([Parameter(Mandatory = $true)][ValidateSet('breaking', 'non-breaking', 'patch')][string]$ChangeType)

    switch ($ChangeType) {
        'breaking'     { return 'breaking' }
        'non-breaking' { return 'non-breaking' }
        'patch'        { return 'patch' }
    }
}

# Builds the cascade announcement line. ExposingChangeType is what we apply to
# dependents that re-export the target's types in their public API;
# NonExposingChangeType is what we apply to internal-only consumers (today: always
# 'patch'). When the two are identical (i.e. the target itself is being
# released as a patch, so the non-exposing cascade-applied change cannot go
# any lower), the parenthetical clause is suppressed entirely — saying "or
# patch if no API exposure" would just repeat the headline label.
function Format-CascadeAnnouncement {
    param(
        [Parameter(Mandatory = $true)][ValidateSet('breaking', 'non-breaking', 'patch')][string]$ExposingChangeType,
        [Parameter(Mandatory = $true)][ValidateSet('breaking', 'non-breaking', 'patch')][string]$NonExposingChangeType,
        [Parameter(Mandatory = $true)][string]$TargetPackageName,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$DependentNames
    )

    $count = @($DependentNames).Count
    $noun  = if ($count -eq 1) { 'dependent package' } else { 'dependent packages' }

    $headlineLabel = Get-ChangeTypeLabel -ChangeType $ExposingChangeType

    if ($ExposingChangeType -eq $NonExposingChangeType) {
        $parenthetical = ''
    } else {
        $downgradeLabel = Get-ShortChangeTypeLabel -ChangeType $NonExposingChangeType
        $parenthetical = " (or $downgradeLabel if no API exposure of ``$TargetPackageName``)"
    }

    return "🔗 Cascading release to $count $noun as $headlineLabel$parenthetical`: $($DependentNames -join ', ')"
}

# Per-dependent "  • <name> -> <change-type-label> (<why>)" line printed
# under the cascade announcement. The label uses the SHORT semantic change-
# type form (breaking / non-breaking / patch) so the readout matches the
# announcement's vocabulary. The why-clause tells the user what drove the
# cascade-applied change-type choice — public-API exposure vs. internal use —
# so the reader can quickly sanity-check whether the inferred exposure
# matches their mental model.
function Format-CascadeDependentLine {
    param(
        [Parameter(Mandatory = $true)][string]$DependentName,
        [Parameter(Mandatory = $true)][ValidateSet('breaking', 'non-breaking', 'patch')][string]$ChangeType,
        [Parameter(Mandatory = $true)][bool]$ExposesTarget
    )

    $label   = Get-ShortChangeTypeLabel -ChangeType $ChangeType
    $why     = if ($ExposesTarget) { 'exposes target in public API' } else { 'internal use only' }
    return "  • $DependentName -> $label ($why)"
}

# Pure formatter for the "Detected pending releases ..." block printed
# at the top of Invoke-ReleaseMain. Each pending record is a [pscustomobject] with
# Name, BaseVersion, CurrentVersion (Get-PendingReleases produces these in stable
# Folder order). Returns '' when there are no pending releases so the caller can
# unconditionally print and rely on Write-Host to no-op on empty input.
#
# A pending release is any package whose on-disk version differs from its
# BaseRef version, irrespective of whether that change has been committed in
# the working branch. See docs/releasing.md for the rationale: pending
# status is determined by comparison with BaseRef, never by working-tree
# vs HEAD state.
#
# Format:
#   Detected pending releases and included in analysis data set:
#      <name1> <base1> -> <current1>
#      <name2> <base2> -> <current2>
function Format-PendingReleasesAnnouncement {
    param(
        [Parameter(Mandatory = $true)][AllowNull()][AllowEmptyCollection()]$Pending
    )

    if ($null -eq $Pending) { return '' }
    $items = @($Pending)
    if ($items.Count -eq 0) { return '' }

    $lines = @('Detected pending releases and included in analysis data set:')
    foreach ($entry in $items) {
        $lines += "   $($entry.Name) $($entry.BaseVersion) -> $($entry.CurrentVersion)"
    }
    return ($lines -join [Environment]::NewLine)
}

# Runs the version increment + cascade toward dependents for a single target package.
# Returns the augmented $releases array. Equivalent to the legacy inline body, but
# factored so the post-release scan can invoke it recursively for transitive
# dependencies the user agrees to release.
function Invoke-ReleaseFlow {
    param(
        [Parameter(Mandatory = $true)][string]$PackageName,
        [Parameter(Mandatory = $false)][string]$Version = '',
        [Parameter(Mandatory = $false)][string]$ChangeType = '',
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $false)][string]$PrBaseUrl,
        [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$BaseRef
    )

    $packageFolder    = Join-Path $RepoRoot 'crates' $PackageName
    $packageCargoToml = Join-Path $packageFolder 'Cargo.toml'
    $changelogFile  = Join-Path $packageFolder 'CHANGELOG.md'

    $currentVersion = Get-CurrentVersion -cargoTomlPath $packageCargoToml
    if ([string]::IsNullOrWhiteSpace($currentVersion)) {
        Write-Error "Failed to determine current version for '$PackageName'. Aborting."
        Exit 1
    }

    $baseVersion = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $PackageName

    # Re-invocation on a primary target that already has a pending in-branch
    # version change (committed or uncommitted — both are equivalent until
    # the branch merges into the base ref, so this code path makes no
    # distinction between them; see docs/releasing.md). Typical sources of
    # the pending change: an earlier `release-crate.ps1` invocation in the
    # same PR, or a cascade-applied bump from a previous run.
    # Mirrors the base-relative no-op/upgrade logic Invoke-CascadeStep already
    # applies to dependents: compute the version this invocation WOULD have
    # produced starting from the base-ref version, and compare with the on-disk
    # current version.
    $isPendingPrimary = (-not [string]::IsNullOrEmpty($baseVersion)) -and ($currentVersion -ne $baseVersion)

    if ($isPendingPrimary) {
        $requiredVersion = if (-not [string]::IsNullOrEmpty($Version)) {
            $Version
        } elseif (-not [string]::IsNullOrEmpty($ChangeType)) {
            Get-NextVersion -currentVersion $baseVersion -ChangeType $ChangeType
        } else {
            # Default change type (no -Version, no -ChangeType) matches
            # Invoke-PackageRelease's internal default of 'non-breaking'.
            # Keeps re-invocation idempotent with the initial bare call.
            Get-NextVersion -currentVersion $baseVersion -ChangeType 'non-breaking'
        }

        $cmp = Compare-SemanticVersions -version1 $currentVersion -version2 $requiredVersion

        if ($cmp -gt 0 -and -not [string]::IsNullOrEmpty($Version)) {
            # Explicit -Version asks for something lower than the pending current.
            # Treat as a likely user mistake (typo, stale flag) rather than silently
            # no-opping into the higher pending version.
            Write-Error "Cannot release '$PackageName' as v${Version}: package is already pending at v$currentVersion (base v$baseVersion). Explicit -Version downgrades are not supported."
            Exit 1
        }

        if ($cmp -ge 0) {
            # No-op for the primary. The on-disk Cargo.toml + changelog from the
            # prior invocation already reflect the intended release. Cascade still
            # runs because dependents may benefit from another idempotent pass.
            Write-Host "ℹ️  '$PackageName' already pending at v$currentVersion (base v$baseVersion); skipping primary release." -ForegroundColor DarkGray
            $oldVersion = $baseVersion
            $newVersion = $currentVersion

            # Cascade-applied change type derives from the EFFECTIVE base→current
            # transition, not the user-requested change type. Otherwise a re-
            # invocation with a weaker -Change (e.g. Patch on a previously
            # non-breaking primary) would under-cascade dependents that need
            # the stronger change to stay compatible with the on-disk API
            # changes.
            $cascadeChangeType = Get-ChangeTypeFromVersions -oldVersion $baseVersion -newVersion $currentVersion
        } else {
            # cmp < 0: requested release would escalate the primary above its
            # current pending version. Re-stamp Cargo.toml / root Cargo.toml /
            # CHANGELOG header in place — body content (commit bullets + cascade
            # bullets from the prior pending pass) carries forward verbatim,
            # because the only thing that changed is the user's change-type
            # intent.
            Write-Host "⬆️  Escalating pending release of '$PackageName' from v$currentVersion to v$requiredVersion (base v$baseVersion)." -ForegroundColor Yellow
            Update-PendingReleaseVersion -PackageName $PackageName -PackageFolder $packageFolder `
                -RootCargoToml $RootCargoToml -OldVersion $currentVersion -NewVersion $requiredVersion | Out-Null
            Invalidate-WorkspaceMetadataCache

            $oldVersion = $baseVersion
            $newVersion = $requiredVersion
            # Cascade-applied change type derives from the effective base →
            # escalated transition so dependents (which may already be cascade-
            # released from the prior pass) get re-evaluated against the new
            # requirement.
            $cascadeChangeType = Get-ChangeTypeFromVersions -oldVersion $baseVersion -newVersion $requiredVersion
        }
    } else {
        $oldVersion = $currentVersion
        $newVersion = Invoke-PackageRelease -packageName $PackageName -packageFolder $packageFolder `
            -packageCargoToml $packageCargoToml -rootCargoToml $RootCargoToml -changelogFile $changelogFile `
            -prBaseUrl $PrBaseUrl -version $Version -ChangeType $ChangeType
        Invalidate-WorkspaceMetadataCache

        $cascadeChangeType = if (-not [string]::IsNullOrEmpty($ChangeType)) {
            $ChangeType
        } elseif (-not [string]::IsNullOrEmpty($Version)) {
            Get-ChangeTypeFromVersions -oldVersion $oldVersion -newVersion $newVersion
        } else {
            'non-breaking'
        }
    }

    $releases = @(
        [pscustomobject]@{ Package = $PackageName; OldVersion = $oldVersion; NewVersion = $newVersion }
    )

    $targetIsBreaking = Test-IsBreakingChange -oldVersion $oldVersion -ChangeType $cascadeChangeType
    $exposingCascadeChangeType = if ($targetIsBreaking) { 'breaking' } else { $cascadeChangeType }

    $dependents = @(Get-AllTransitiveDependents -packageName $PackageName -repoRoot $RepoRoot)
    if ($dependents.Count -gt 0) {
        Write-Host ""
        $cascadeMessage = Format-CascadeAnnouncement -ExposingChangeType $exposingCascadeChangeType `
            -NonExposingChangeType 'patch' -TargetPackageName $PackageName -DependentNames $dependents
        Write-Host $cascadeMessage -ForegroundColor Cyan

        $allPackages = Get-WorkspacePackages -repoRoot $RepoRoot
        $targetPackage = $allPackages | Where-Object { $_.Folder -eq $PackageName -or $_.Name -eq $PackageName } | Select-Object -First 1
        $targetPackageName = if ($null -ne $targetPackage) { $targetPackage.Name } else { $PackageName }

        foreach ($dependent in $dependents) {
            $depPackage = $allPackages | Where-Object { $_.Folder -eq $dependent } | Select-Object -First 1
            $exposes = if ($null -ne $depPackage) {
                Test-PackageExposesTarget -dependent $depPackage -targetPackageName $targetPackageName
            } else { $true }

            $dependentChangeType = if ($exposes) { $exposingCascadeChangeType } else { 'patch' }
            Write-Host (Format-CascadeDependentLine -DependentName $dependent -ChangeType $dependentChangeType -ExposesTarget $exposes) -ForegroundColor DarkCyan

            $record = Invoke-CascadeStep -Dependent $dependent -RepoRoot $RepoRoot `
                -RootCargoToml $RootCargoToml -PrBaseUrl $PrBaseUrl `
                -TargetPackageName $PackageName -TargetNewVersion $newVersion `
                -DependentChangeType $dependentChangeType -BaseRef $BaseRef
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
# the non-breaking and patch change types under the same numeric increment
# (0.x.(y+1)) — and on 0.0.x where every change type collapses to 0.0.(x+1).
# When CurrentVersion is unknown, we conservatively return $false so all
# options remain visible.
function Test-IsPatchOptionRedundant {
    param([Parameter(Mandatory = $true)][AllowNull()][AllowEmptyString()][string]$CurrentVersion)

    if ([string]::IsNullOrWhiteSpace($CurrentVersion)) { return $false }
    $nonBreakingNext = Get-NextVersion -currentVersion $CurrentVersion -ChangeType 'non-breaking'
    $patchNext = Get-NextVersion -currentVersion $CurrentVersion -ChangeType 'patch'
    return ($nonBreakingNext -eq $patchNext)
}

# Pure formatter for the per-package menu. Returns a multi-line string ready
# for Write-Host. Returning a string (not host-writing directly) keeps the
# function unit-testable without redirecting Information / Host streams.
#
# Options 3-5 render the *concrete* version transition each choice would
# produce (e.g. "Release as breaking change (0.1.2 -> 0.2.0)"). Get-NextVersion
# is the single source of truth for the version-component math and already
# honours Cargo's 0.x.y semver carve-outs, so the menu always shows the same
# version the release would produce — not a misleading numeric label.
#
# Option 5 (Release as patch) is hidden when it would produce the same numeric
# increment as option 4 (Release as non-breaking change) — see
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
    $changeTypeHints = @{}
    foreach ($kind in @('breaking', 'non-breaking', 'patch')) {
        if ([string]::IsNullOrWhiteSpace($current)) {
            $changeTypeHints[$kind] = "($kind)"
        } else {
            $next = Get-NextVersion -currentVersion $current -ChangeType $kind
            $changeTypeHints[$kind] = "($current -> $next)"
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
    [void]$sb.AppendLine("  3. Release as breaking change $($changeTypeHints['breaking'])")
    [void]$sb.AppendLine("  4. Release as non-breaking change $($changeTypeHints['non-breaking'])")
    if (-not $hidePatch) {
        [void]$sb.AppendLine("  5. Release as patch $($changeTypeHints['patch'])")
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
# baseline (Get-PackageLastReleaseBaseline). When no baseline is found (e.g.
# a never-released package), falls back to `git diff HEAD` and prefixes the
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

    $baseline = Get-PackageLastReleaseBaseline -RepoRoot $RepoRoot -PackageFolder $Folder
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
# 'ignore' | 'breaking' | 'non-breaking' | 'patch' }.
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
        if ($choice -eq '3') { return @{ Action = 'breaking' } }
        if ($choice -eq '4') { return @{ Action = 'non-breaking' } }
        if ($choice -eq '5' -and -not $hidePatch) { return @{ Action = 'patch' } }

        Write-Host "Invalid choice '$choice'. Enter a number from 1 to $maxChoice." -ForegroundColor Yellow
    }
}

# Scans for workspace packages with unreleased modifications (changes newer than the
# package's own last `version =` / `publish =` commit) that are transitively pulled in
# by a release-set member but are not themselves part of the release set, prompting
# the user (when interactive) to optionally release them too.
# Newly-released packages are appended to the release records via [ref].
#
# CASCADE-ORGANIZATION INVARIANTS (see docs/releasing.md "Cascade Organisation Invariants"):
#   A. A cascade toward dependents never introduces items to the user-review
#      queue. A cascade-only target (no pre-existing modifications) must NOT
#      surface — its version increment is mechanical and follows directly
#      from the released dependency. Enforced by snapshotting the
#      modifications set BEFORE the primary release / cascade runs (the
#      snapshot is passed in via -ModifiedSnapshot and forwarded to
#      Get-UnreleasedModifiedDependencies).
#   B. A release-set member is removed from the user-review queue ONLY when
#      its cascade-applied change type is already breaking (semantic maximum)
#      OR when the user has explicitly reviewed it in this run / a prior
#      release-crate.ps1 invocation. Release-set members whose cascade-applied
#      change is non-breaking or patch are SURFACED when they have pre-existing
#      modifications, because the user may want to escalate after reviewing
#      the changes (a cascade toward dependents is only definitive if the
#      ONLY change in that package is the mechanical dependency increment).
#      Enforced jointly by Get-UnreleasedModifiedDependencies (the BFS
#      surfacing rule) and the $reviewedReleaseSet bookkeeping inside this
#      loop.
function Invoke-PostReleaseDepScan {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$BaseRef,
        [Parameter(Mandatory = $true)][ref]$ReleasesRef,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $false)][string]$PrBaseUrl,
        # Caller-captured "has unreleased modifications" hashtable. When provided,
        # used in lieu of re-querying the working tree mid-loop. Required to
        # uphold Invariant A across cascades (the cascade-written Cargo.toml /
        # CHANGELOG.md / README.md edits would otherwise pollute the snapshot
        # and surface cascade-only targets as findings).
        [Parameter(Mandatory = $false)][hashtable]$ModifiedSnapshot,
        # Folders whose release the user has already explicitly decided about
        # — i.e. they must NOT be re-prompted as Invariant B elevation
        # candidates in this scan. Typical contents:
        #   - The primary target of this invocation (the user just chose
        #     `-Change <kind>` for it; no point re-asking).
        #   - The cross-invocation pending-releases set captured before any
        #     work in this run (prior `release-crate.ps1` invocations in the
        #     same PR already settled those).
        # IMPORTANT: this list must NOT include packages that were only
        # cascade-released (no explicit user review). Those must remain
        # eligible for elevation review per Invariant B — accepting one
        # release whose cascade pulls additional modified packages into the
        # release set with a non-breaking change type is exactly the
        # condition under which the user needs a chance to review and
        # potentially elevate the cascade-applied change type.
        [Parameter(Mandatory = $false)][string[]]$PreReviewedFolders
    )

    $isInteractive = Test-InteractiveSession

    # $declined tracks NON-release-set findings the user said "no" to. The
    # "ignore-then-cascade" handoff later removes from this set if a cascade
    # ends up pulling the package into the release set anyway.
    $declined = [System.Collections.Generic.HashSet[string]]::new()

    # $reviewedReleaseSet tracks release-set members whose review (either
    # "elevate" or "ignore — current change type is fine") is complete in
    # this run OR was decided in a prior invocation. Filtering on this set
    # is what prevents the re-prompt loop where a release-set member with
    # a non-breaking or patch cascade-applied change would otherwise keep
    # resurfacing after the user accepts the current change type.
    #
    # IMPORTANT: only the primary target of THIS invocation and packages the
    # user has explicitly decided about in prior invocations are pre-marked
    # here. Cascade-released members of $ReleasesRef are NOT pre-marked —
    # they must remain eligible for Invariant B elevation review when
    # they also have pre-existing modifications.
    $reviewedReleaseSet = [System.Collections.Generic.HashSet[string]]::new()
    if ($null -ne $PreReviewedFolders) {
        foreach ($f in $PreReviewedFolders) { [void]$reviewedReleaseSet.Add($f) }
    }

    # Termination bound: number of published workspace packages. The dep graph is
    # a DAG, so each iteration either grows ($declined ∪ release-set)
    # monotonically or terminates.
    $maxIterations = @(Get-WorkspacePackages -repoRoot $RepoRoot | Where-Object { $_.Published }).Count
    if ($maxIterations -lt 1) { $maxIterations = 1 }

    # Save/restore the temp-diff-paths tracking list so a re-entry into this
    # function (current callers never re-enter, but the helper API allows it)
    # cannot clobber an outer run's list.
    $prevTempPaths = $script:TempPackageDiffPaths
    $script:TempPackageDiffPaths = [System.Collections.Generic.List[string]]::new()

    try {
        # $queue is recomputed only when needed:
        #   - On entry (first iteration).
        #   - After any decision that actually changed on-disk / release-set
        #     state (i.e. user accepted a release, which then runs a
        #     cascade that may introduce new version changes).
        # After an 'ignore' decision nothing on disk or in git changes, so
        # we just filter the previous queue by the updated $declined /
        # $reviewedReleaseSet sets — avoiding ~120 git spawns and a cargo
        # metadata recompute per click.
        $queue = $null
        $hasEverComputedQueue = $false
        for ($iter = 0; $iter -lt $maxIterations; $iter++) {
            if ($null -eq $queue) {
                # Full recompute path: BFS over cargo metadata + per-package
                # diffs. The session-scoped baseline / committed-changes /
                # version-at-ref caches keep N×git calls down to a single
                # warm-up burst on the first iteration; later recomputes
                # (post-release) hit the caches except for the working-tree
                # diff and ls-files calls (2 git invocations total).
                if ($isInteractive) {
                    Write-Host ''
                    Write-Host '🔍 Analyzing packages for unreleased modifications...' -ForegroundColor Cyan
                }

                $queue = @(
                    @(Get-UnreleasedModifiedDependencies -RepoRoot $RepoRoot -BaseRef $BaseRef -ModifiedSnapshot $ModifiedSnapshot) |
                        Where-Object {
                            -not $declined.Contains($_.Folder) -and
                            -not $reviewedReleaseSet.Contains($_.Folder)
                        }
                )

                if ($queue.Count -eq 0) {
                    if (-not $hasEverComputedQueue) {
                        Write-Host ""
                        Write-Host "✅ No modified-but-unreleased workspace dependencies detected." -ForegroundColor Green
                    }
                    return
                }
                $hasEverComputedQueue = $true
            } else {
                # Cheap path: previous decision was 'ignore', so the only
                # change since the last queue snapshot is $declined or
                # $reviewedReleaseSet gaining one entry. The BFS output would
                # be identical modulo that one filter, so re-filter and skip
                # the spawn storm.
                $queue = @($queue | Where-Object {
                    -not $declined.Contains($_.Folder) -and
                    -not $reviewedReleaseSet.Contains($_.Folder)
                })
                if ($queue.Count -eq 0) { return }
            }

            if (-not $isInteractive) {
                # Non-interactive parity: emit the full pending list once and bail,
                # marking everything as declined / reviewed. The reviewer-facing
                # comment from check-unreleased-dependencies.ps1 will flag the
                # same set.
                $notInReleaseSet = @($queue | Where-Object { -not $_.InReleaseSet })
                $inReleaseSet    = @($queue | Where-Object { $_.InReleaseSet })

                Write-Host ""
                if ($notInReleaseSet.Count -gt 0) {
                    Write-Host '⚠️  The following workspace packages have unreleased modifications (changes newer than their last `version =` / `publish =` commit) and are NOT part of this release:' -ForegroundColor Yellow
                    foreach ($finding in $notInReleaseSet) {
                        Write-Host "  • $($finding.Folder)" -ForegroundColor Yellow
                        Write-Host '      potentially affected dependency chains:' -ForegroundColor DarkGray
                        foreach ($chain in $finding.DependencyChains) {
                            Write-Host "        $($chain -join ' -> ')" -ForegroundColor DarkGray
                        }
                    }
                }
                if ($inReleaseSet.Count -gt 0) {
                    Write-Host '⚠️  The following workspace packages are being released as part of this PR with a non-breaking cascade-applied version change, BUT also have pre-existing modifications that may warrant a more impactful change type (e.g. breaking). A reviewer should confirm the cascade-applied change type is sufficient:' -ForegroundColor Yellow
                    foreach ($finding in $inReleaseSet) {
                        Write-Host "  • $($finding.Folder)" -ForegroundColor Yellow
                        Write-Host '      cascade-pulled in via:' -ForegroundColor DarkGray
                        foreach ($chain in $finding.DependencyChains) {
                            Write-Host "        $($chain -join ' -> ')" -ForegroundColor DarkGray
                        }
                    }
                }
                Write-Warning "Non-interactive session: leaving the above packages as-is. Reviewer should confirm the choices are appropriate."
                foreach ($finding in $queue) {
                    if ($finding.InReleaseSet) {
                        [void]$reviewedReleaseSet.Add($finding.Folder)
                    } else {
                        [void]$declined.Add($finding.Folder)
                    }
                }
                return
            }

            # Process one finding per outer iteration. Cascade-released findings
            # naturally drop out of the next iteration's queue because the
            # cascade commits their version changes, so they no longer appear
            # in the modified-but-unreleased set on the next BFS snapshot.
            $next       = $queue[0]
            $remaining  = $queue.Count - 1
            $decision   = Get-PackageReleaseDecision -Finding $next -RemainingCount $remaining -RepoRoot $RepoRoot

            if ($decision.Action -eq 'ignore') {
                if ($next.InReleaseSet) {
                    Write-Host "  Keeping '$($next.Folder)' at its current cascade-applied version; reviewer should confirm no further elevation is needed." -ForegroundColor DarkGray
                    [void]$reviewedReleaseSet.Add($next.Folder)
                } else {
                    Write-Host "  Leaving '$($next.Folder)' unreleased; reviewer should confirm the change is immaterial." -ForegroundColor DarkGray
                    [void]$declined.Add($next.Folder)
                }
                # Keep $queue intact so the next iteration takes the cheap path.
                continue
            }

            Write-Host ""
            Write-Host "🚀 Releasing '$($next.Folder)' as $($decision.Action)..." -ForegroundColor Cyan
            $nestedReleases = @(Invoke-ReleaseFlow -PackageName $next.Folder -ChangeType $decision.Action `
                -RepoRoot $RepoRoot -RootCargoToml $RootCargoToml -PrBaseUrl $PrBaseUrl -BaseRef $BaseRef)

            # Merge nested release records into the running set. A package may already
            # appear (e.g., it was a cascade-toward-dependents target of the initial release)
            # and the nested cascade may have upgraded it further — preserve the
            # original OldVersion (the pre-PR baseline) and adopt the latest NewVersion
            # so Show-ReleaseSummary and the final commit message reflect on-disk state.
            #
            # IMPORTANT: we mark only $next.Folder (the package the user just
            # explicitly decided about) as reviewed — NOT the cascade-released
            # members of $nestedReleases. Cascade-released packages with
            # pre-existing modifications must remain eligible for Invariant B
            # elevation review on the next iteration: a cascade toward
            # dependents is only definitive when the ONLY change in the
            # cascaded package is the mechanical dependency increment.
            foreach ($r in $nestedReleases) {
                # If cascade pulled in a package the user previously chose to ignore,
                # surface that so they're not confused why it appears in the release
                # summary, and update $declined to reflect reality.
                if ($declined.Contains($r.Package)) {
                    Write-Host "ℹ️  Previously ignored package '$($r.Package)' was cascade-released because '$($next.Folder)' was released." -ForegroundColor DarkCyan
                    [void]$declined.Remove($r.Package)
                }

                $existing = $ReleasesRef.Value | Where-Object { $_.Package -eq $r.Package } | Select-Object -First 1
                if ($null -eq $existing) {
                    $ReleasesRef.Value += $r
                } else {
                    $existing.NewVersion = $r.NewVersion
                }
            }

            # Mark ONLY the explicitly-decided package as reviewed. The cascade
            # may have written new versions for other dependents but those
            # decisions were mechanical — if they happen to also have
            # pre-existing modifications, the next BFS iteration will surface
            # them for Invariant B elevation review.
            [void]$reviewedReleaseSet.Add($next.Folder)

            # The cascade just edited Cargo.toml files; force a full BFS
            # recompute next iteration so newly version-incremented packages
            # drop off the findings list (and any of their committed-but-
            # still-unreleased transitive dependencies surface).
            $queue = $null
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

# Translates a semantic -Change value into the internal (ChangeType, Version)
# tuple the rest of the release flow expects. The script's user-facing
# vocabulary is intent-based (Breaking / NonBreaking / Patch / 1.0) because
# that's how releasers reason about the change. The internal vocabulary
# uses the same semantic terms (breaking / non-breaking / patch) — only
# the wire format differs (PascalCase on the CLI to match PowerShell
# parameter conventions, lower-kebab-case internally as canonical enum
# values consumed by Get-NextVersion and friends).
#
# Returned object has exactly one of { ChangeType, Version } populated:
#   - Breaking    → ChangeType='breaking'     Version=''
#   - NonBreaking → ChangeType='non-breaking' Version=''
#   - Patch       → ChangeType='patch'        Version=''
#   - 1.0         → ChangeType=''             Version='1.0.0'  (one-time graduation)
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
        'Breaking'    { return [pscustomobject]@{ ChangeType = 'breaking';     Version = '' } }
        'NonBreaking' { return [pscustomobject]@{ ChangeType = 'non-breaking'; Version = '' } }
        'Patch'       { return [pscustomobject]@{ ChangeType = 'patch';        Version = '' } }
        '1.0' {
            # Force array context — see Compare-SemanticVersions for the rationale.
            $parts = @($CurrentVersion.Split('.') | ForEach-Object { [int]$_ })
            while ($parts.Count -lt 3) { $parts += 0 }
            if ($parts[0] -ge 1) {
                throw "The '-Change 1.0' option is for the one-time 0.x → 1.0.0 graduation event. Current version '$CurrentVersion' is already at 1.x or higher; use '-Change Breaking' instead."
            }
            return [pscustomobject]@{ ChangeType = ''; Version = '1.0.0' }
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
        [Parameter(Mandatory = $true)][string]$PackageName,
        [Parameter(Mandatory = $false)][string]$Version,
        [Parameter(Mandatory = $false)][ValidateSet('Breaking', 'NonBreaking', 'Patch', '1.0')][string]$Change,
        [Parameter(Mandatory = $false)][ValidateNotNullOrEmpty()][string]$BaseRef = 'origin/main'
    )

    # 1. INPUT VALIDATION
    if (-not (Test-ValidPackageName -packageName $PackageName)) {
        Write-Error "Invalid package name '$PackageName'. Package names must contain only letters, numbers, hyphens, and underscores, cannot start or end with hyphen, and must be 64 characters or less."
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

    $packageFolder = Join-Path $repoRoot 'crates' $PackageName
    if (-not (Test-Path $packageFolder)) {
        Write-Error "Package folder not found at '$packageFolder'. Please check the PackageName."
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
    $packageCargoToml = Join-Path $packageFolder "Cargo.toml"
    $rootCargoToml = Join-Path $repoRoot "Cargo.toml"

    if ((-not (Test-Path $packageCargoToml)) -or (-not (Test-Path $rootCargoToml))) {
        Write-Error "Could not find Cargo.toml file in the package folder or repository root."
        Exit 1
    }

    # 5. RESOLVE BASE REF (best-effort fetch + validate)
    # Done before -Change / -Version validation so we can detect cross-invocation
    # pending releases and make those validations base-relative (otherwise a
    # re-invocation on an already-pending package — e.g. `-Change 1.0` on a
    # package already pending at v1.0.0, or `-Version X` where X equals the
    # current pending version — would spuriously fail the on-disk comparison).
    # BaseRef is mandatory (defaulting to 'origin/main'); the dependency- and
    # dependents-scanning logic always runs and cannot be skipped.
    $resolvedBaseRef = $BaseRef
    if ($resolvedBaseRef -match '^origin/(.+)$') {
        $branch = $matches[1]
        try {
            Invoke-Git -Arguments @('fetch', '--no-tags', 'origin', "+refs/heads/${branch}:refs/remotes/origin/${branch}") -RepoRoot $repoRoot.Path | Out-Null
        } catch {
            Write-Warning "git fetch for '$resolvedBaseRef' failed: $_"
        }
    }
    if (-not (Test-GitRef -Ref $resolvedBaseRef -RepoRoot $repoRoot.Path)) {
        Write-Error "Could not resolve base ref '$resolvedBaseRef'. Pass -BaseRef <ref> with a ref that 'git rev-parse' can resolve, or ensure 'origin/main' is fetchable."
        Exit 1
    }

    # 6. ANNOUNCE PENDING RELEASES
    # Helps the user notice prior `release-crate.ps1` runs whose version
    # changes are still in this branch (committed or uncommitted — both are
    # equivalent until the branch merges into the base ref), so they
    # understand why the analysis treats those packages as already-
    # incremented.
    $pendingReleases = @(Get-PendingReleases -RepoRoot $repoRoot.Path -BaseRef $resolvedBaseRef)
    if ($pendingReleases.Count -gt 0) {
        Write-Host ""
        Write-Host (Format-PendingReleasesAnnouncement -Pending $pendingReleases) -ForegroundColor DarkGray
    }

    # Determine whether the primary target is among the pending set. When it is,
    # subsequent validation uses BaseVersion (not on-disk current) as the anchor
    # so this invocation is base-relative — mirroring Invoke-CascadeStep's
    # treatment of already-version-incremented dependents.
    $primaryPending = $pendingReleases | Where-Object { $_.Folder -eq $PackageName } | Select-Object -First 1

    # 7. RESOLVE -Change INTO INTERNAL ($ChangeType, $Version)
    # The CLI surface uses PascalCase semantic vocabulary (Breaking /
    # NonBreaking / Patch / 1.0) to match PowerShell parameter conventions;
    # below this point the internal canonical enum uses the same semantic
    # terms in lower-kebab-case (breaking / non-breaking / patch). The 1.0
    # graduation translates into an explicit -Version 1.0.0; everything else
    # translates into the matching change-type value.
    $changeType = ''
    if (-not [string]::IsNullOrEmpty($Change)) {
        # Use BaseVersion (when pending) so a re-invocation of `-Change 1.0` on
        # an already-graduated pending package idempotently no-ops instead of
        # throwing "already at 1.x" from on-disk inspection. Only the 1.0
        # branch of Resolve-ReleaseSpecFromChange consults this version.
        $versionForChangeCheck = if ($null -ne $primaryPending) {
            $primaryPending.BaseVersion
        } else {
            Get-CurrentVersion -cargoTomlPath $packageCargoToml
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
        $changeType = $spec.ChangeType
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
            Get-CurrentVersion -cargoTomlPath $packageCargoToml
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
        # Capture the modifications snapshot BEFORE running the release flow.
        # This is what upholds Invariant A: any cascade-driven Cargo.toml /
        # CHANGELOG.md / README.md writes performed by Invoke-ReleaseFlow (or
        # by subsequent cascades inside Invoke-PostReleaseDepScan) would
        # otherwise pollute Get-PackagesWithUnreleasedChanges's working-tree
        # query and cause cascade-only targets to surface as findings.
        $preReleaseModifications = Get-PackagesWithUnreleasedChanges -RepoRoot $repoRoot.Path

        # Cross-invocation deduplication: prior `release-crate.ps1` runs in
        # this branch may have left version changes in the working tree that
        # are NOT a concern for the user this time around. Pre-mark every
        # such pending package (except the primary target of this run, which
        # the user is actively re-deciding) as "already reviewed" so the
        # post-release scan doesn't re-prompt for them. The primary target
        # of THIS run is also pre-marked — the user has just chosen
        # `-Change <kind>` for it; re-asking would be redundant.
        $preReviewedFolders = @($pendingReleases | Where-Object { $_.Folder -ne $PackageName } | ForEach-Object { $_.Folder }) + @($PackageName)

        $releases = @(Invoke-ReleaseFlow -PackageName $PackageName -Version $Version -ChangeType $changeType `
            -RepoRoot $repoRoot.Path -RootCargoToml $rootCargoToml -PrBaseUrl $prBaseUrl -BaseRef $resolvedBaseRef)

        # Scan for modified-but-unreleased workspace dependencies and prompt the user.
        # Newly-released packages are appended to $releases via the [ref].
        Invoke-PostReleaseDepScan -RepoRoot $repoRoot.Path -BaseRef $resolvedBaseRef `
            -ReleasesRef ([ref]$releases) -RootCargoToml $rootCargoToml -PrBaseUrl $prBaseUrl `
            -ModifiedSnapshot $preReleaseModifications `
            -PreReviewedFolders $preReviewedFolders

        Invoke-WorkspaceCheck -RepoRoot $repoRoot.Path

        Show-ReleaseSummary -releases $releases
        Show-FinalMessage -PackageName $PackageName -Releases $releases

        return ,$releases
    }
    catch {
        Write-Error "Script failed: $_"
        Exit 1
    }
}
