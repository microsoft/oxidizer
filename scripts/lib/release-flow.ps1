# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Release-flow library: helpers and orchestration for scripts/release-packages.ps1.

.DESCRIPTION
    Owns the orchestration helpers, changelog formatters, and the
    Invoke-ReleasePackagesMain entrypoint that drives the full package-release
    workflow. scripts/release-packages.ps1 is a thin CLI shell that dot-sources
    this library and calls Invoke-ReleasePackagesMain.

    This file is NOT an entrypoint. It only defines functions and module-scoped
    configuration; dot-source it from another script (or from Pester tests) to
    consume its API.

    Depends on scripts/lib/releasing.ps1 (which it dot-sources at the top so
    consumers only need to source this file).
#>

# --- DOT-SOURCE SHARED LIBRARY ---
#
# scripts/lib/releasing.ps1 owns the lower-level reusable building blocks used by
# the release flow below:
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
#   - breaking, nonbreaking, patch        (case-insensitive change-type keywords)
#   - <major>.<minor>.<patch>[-<prerelease>][+<build>]
#                                         (strict SemVer 2.0 pin — always
#                                          exactly three numeric components,
#                                          no leading zeros; optional
#                                          pre-release identifier and optional
#                                          build metadata. Examples: '1.0.0',
#                                          '2.5.0', '1.0.0-rc.1',
#                                          '0.1.0-beta.2+meta')
#
# Returns an array of pscustomobject entries, one per token:
#
#   @{
#     Name                   = '<as-typed>'             # case preserved
#     RequestedChangeType    = 'breaking'|'non-breaking'|'patch'|$null
#     RequestedTargetVersion = '1.2.3'|$null            # set only for explicit semver pin
#     RawToken               = '<original token>'
#   }
#
# Explicit semver pins (including '1.0.0') are passed through verbatim as
# RequestedTargetVersion; the resolver later rejects pins that are not
# strictly greater than the package's current version.
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
#     One- or two-component version forms ('1', '1.2') are rejected.
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
            throw "Invalid package name '$name' in token '$raw'. Package names must contain only letters, numbers, hyphens, and underscores; must not start or end with a hyphen or underscore; and must be 64 characters or less."
        }

        if (-not $seenNames.Add($name)) {
            throw "Duplicate package name '$name' in -Packages list. Each package may appear at most once; release each package with a single combined change type."
        }

        $requestedChangeType    = $null
        $requestedTargetVersion = $null

        switch -CaseSensitive ($changeSpec.ToLowerInvariant()) {
            'breaking'    { $requestedChangeType = 'breaking';     break }
            'nonbreaking' { $requestedChangeType = 'non-breaking'; break }
            'patch'       { $requestedChangeType = 'patch';        break }
            default {
                # Strict SemVer 2.0 — three numeric components, optional
                # pre-release identifier (-...), optional build metadata
                # (+...). 1- or 2-component forms like 'foo@1' or 'foo@1.2'
                # are intentionally rejected; pinning a release requires
                # full disambiguation.
                if ($script:SemanticVersionRegex.IsMatch($changeSpec)) {
                    $requestedTargetVersion = $changeSpec
                } else {
                    throw "Invalid change specifier '$changeSpec' in token '$raw'. Expected one of: 'breaking', 'nonbreaking', 'patch', or an explicit SemVer 2.0 version with all three components (e.g. '1.0.0', '2.5.0', '1.0.0-rc.1', '1.0.0-beta.2+meta'). One- or two-component forms like '1' or '1.2' are not accepted."
                }
            }
        }

        $results.Add([pscustomobject]@{
            Name                   = $name
            RequestedChangeType    = $requestedChangeType
            RequestedTargetVersion = $requestedTargetVersion
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
#   -Force            : if set, an explicit version pin that numerically
#                       undershoots the cascade-required version is honored
#                       verbatim instead of throwing. EffectiveChangeType is
#                       still upgraded so dependent cascade decisions are
#                       correct; PinHonoredAgainstCascade is set so callers
#                       (and Show-ReleasePlan) can warn the user. -Force does
#                       NOT relax the always-fatal "pin is not strictly
#                       greater than the current on-disk version" check.
#
# Returns: an array of pscustomobject entries, one per resolved package:
#
#   @{
#     Folder                    = '<crate folder>'
#     Name                      = '<cargo package name>'  # may differ from Folder
#     CurrentVersion            = '<baseline version>'
#     RequestedChangeType       = 'breaking'|'non-breaking'|'patch'|$null   # null for cascade-source
#     RequestedTargetVersion    = '<pin>'|$null                              # null when not pinned
#     EffectiveChangeType       = 'breaking'|'non-breaking'|'patch'         # after cascade resolution
#     EffectiveTargetVersion    = '<version>'                                # after cascade resolution
#     Source                    = 'user'|'cascade'
#     AutoUpgraded              = $true|$false   # user-source entry strengthened by cascade
#     PinHonoredAgainstCascade  = $true|$false   # -Force kept an explicit pin below cascade-required version
#     CascadeReasons            = [List<{Target,Version,Breaking}>]          # one per (target → dep) edge
#     RawToken                  = '<original token>'|$null                   # null for cascade-source
#   }
#
# Resolution algorithm:
#   1. Seed every token as a user-source entry. Reject:
#        - tokens for non-workspace packages
#        - tokens for unpublished workspace packages
#        - explicit version pins not strictly greater than the current version
#   2. For each user-source entry, BFS via
#      Get-TransitivePublishedDependentsFromBaseline to collect all published
#      transitive dependents. For each dependent compute the cascade-applied
#      change type from exposing/non-exposing semantics, and either:
#        - upgrade an existing entry (rank-ordered: patch < non-breaking <
#          breaking). For user-source entries with -Change keyword,
#          auto-upgrade silently and set AutoUpgraded=$true. For user-source
#          entries with an explicit version pin, throw if the pin would
#          numerically undershoot the cascade-required version (or, with
#          -Force, honor the pin verbatim, upgrade the change-type tag, and
#          set PinHonoredAgainstCascade=$true); otherwise honour the pin and
#          bump only the change-type tag.
#        - or create a new cascade-source entry.
#      Cascade reasons are recorded per (target → dep) edge with dedup by
#      target name (re-encountering an edge for an already-strengthened target
#      overwrites the prior reason in place).
#
# Note: cascade is one-level. The set of dependents reachable from a user
# target is the transitive published dependents BFS, but the cascade-applied
# change type for each dependent is derived from exposure of the USER TARGET
# (not of any intermediate). Tightening the analysis is out of scope for the
# redesign.
#
# After the iteration loop finishes, a final normalisation pass re-points
# every recorded CascadeReason.Version to the target entry's FINAL
# EffectiveTargetVersion. This is necessary because the version captured
# during a target's outer-loop iteration may be superseded later if that
# same target is also a dependent of yet another user-source target whose
# iteration strengthens it. Without normalisation, consumers
# (Write-Changelog "Now requires <v> of <pkg>" bullets) would print the
# stale pre-bump version. The companion Breaking flag is intentionally
# left untouched — recomputing it without also re-iterating the dependent
# would conflate "the target became breaking" with "this edge contributes
# breaking", which under one-level cascade semantics it does not.
function Resolve-ReleaseSet {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$ParsedTokens,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$WorkspaceBaseline,
        [Parameter(Mandatory = $false)][switch]$Force
    )

    if ($null -eq $ParsedTokens -or @($ParsedTokens).Count -eq 0) {
        throw "Resolve-ReleaseSet: ParsedTokens is empty. Parse-ReleaseTokens should reject empty input earlier."
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
            throw "Internal error: package '$($pkg.Folder)' resolved twice from -Packages (token '$($req.RawToken)'). Parse-ReleaseTokens should have rejected the duplicate earlier."
        }

        $currentVersion = $pkg.Version

        if (-not [string]::IsNullOrEmpty($req.RequestedTargetVersion)) {
            $target = $req.RequestedTargetVersion
            $cmp = Compare-SemanticVersions -version1 $target -version2 $currentVersion
            if ($cmp -le 0) {
                throw "Cannot release '$($pkg.Folder)' as v$($target): package is already at v$currentVersion. Explicit version pins must be strictly greater than the current version. Token: '$($req.RawToken)'."
            }
            $effectiveChangeType    = Get-ChangeTypeFromVersions -oldVersion $currentVersion -newVersion $target
            $effectiveTargetVersion = $target
        } else {
            $effectiveChangeType    = $req.RequestedChangeType
            $effectiveTargetVersion = Get-NextVersion -currentVersion $currentVersion -ChangeType $effectiveChangeType
        }

        $resolved[$pkg.Folder] = [pscustomobject]@{
            Folder                   = $pkg.Folder
            Name                     = $pkg.Name
            CurrentVersion           = $currentVersion
            RequestedChangeType      = $req.RequestedChangeType
            RequestedTargetVersion   = $req.RequestedTargetVersion
            EffectiveChangeType      = $effectiveChangeType
            EffectiveTargetVersion   = $effectiveTargetVersion
            Source                   = 'user'
            AutoUpgraded             = $false
            PinHonoredAgainstCascade = $false
            CascadeReasons           = New-Object 'System.Collections.Generic.List[object]'
            RawToken                 = $req.RawToken
        }
    }

    # Snapshot the user-source folder names before cascade adds cascade-source
    # entries — cascade-source entries are not themselves iterated for further
    # cascades (one-level cascade semantics).
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
                        # has to revise their request — unless -Force was set,
                        # in which case we honor the pin verbatim, still bump
                        # the EffectiveChangeType tag, and flag the entry so
                        # the user sees a clear warning at plan-display time.
                        $cmpPin = Compare-SemanticVersions -version1 $existing.RequestedTargetVersion -version2 $cascadeRequiredVersion
                        if ($cmpPin -lt 0) {
                            if ($Force) {
                                $reasonsNames = ($existing.CascadeReasons | ForEach-Object { $_.Target } | Sort-Object -Unique) -join ', '
                                Write-Warning "-Force: honoring explicit pin v$($existing.RequestedTargetVersion) on '$($existing.Folder)' even though cascade requires at least v$cascadeRequiredVersion (cascade sources: $reasonsNames). The package's EffectiveChangeType tag is upgraded to '$dependentChangeType' but the version on disk will be v$($existing.RequestedTargetVersion). Consumers may break."
                                $existing.EffectiveChangeType       = $dependentChangeType
                                $existing.PinHonoredAgainstCascade  = $true
                            } else {
                                $reasonsNames = ($existing.CascadeReasons | ForEach-Object { $_.Target } | Sort-Object -Unique) -join ', '
                                throw "Cannot release '$($existing.Folder)' as v$($existing.RequestedTargetVersion): cascade requires at least v$cascadeRequiredVersion because of changes in: $reasonsNames. Specify a higher version pin, use a change-type keyword, or pass -Force to honor the pin verbatim (consumers may break)."
                            }
                        } else {
                            # Pin still satisfies. Bump the EffectiveChangeType tag
                            # (so cascade re-exposure decisions for this entry's
                            # own dependents — if we iterated them, which we don't
                            # at present — would be correct) but keep the pin as
                            # the version.
                            $existing.EffectiveChangeType = $dependentChangeType
                        }
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
                    Folder                   = $depPkg.Folder
                    Name                     = $depPkg.Name
                    CurrentVersion           = $depPkg.Version
                    RequestedChangeType      = $null
                    RequestedTargetVersion   = $null
                    EffectiveChangeType      = $dependentChangeType
                    EffectiveTargetVersion   = Get-NextVersion -currentVersion $depPkg.Version -ChangeType $dependentChangeType
                    Source                   = 'cascade'
                    AutoUpgraded             = $false
                    PinHonoredAgainstCascade = $false
                    CascadeReasons           = New-Object 'System.Collections.Generic.List[object]'
                    RawToken                 = $null
                }
                $newEntry.CascadeReasons.Add($cascadeReason)
                $resolved[$depPkg.Folder] = $newEntry
            }
        }
    }

    # Normalise CascadeReason.Version to each target entry's FINAL
    # EffectiveTargetVersion. A target X's iteration captures the version
    # value live at that moment when emitting reasons onto its dependents.
    # If a later outer-loop iteration (some user-source Y whose BFS reaches
    # X) bumps X.EffectiveTargetVersion via the rank-comparison branch
    # above, the reasons X already emitted are now stale. Re-point them
    # here so changelog "Now requires <Version> of <Target>" bullets render
    # the post-cascade plan rather than an intermediate snapshot. Breaking
    # is intentionally NOT recomputed (see header comment).
    $finalVersionByName = @{}
    foreach ($e in $resolved.Values) { $finalVersionByName[$e.Name] = $e.EffectiveTargetVersion }
    foreach ($e in $resolved.Values) {
        for ($i = 0; $i -lt $e.CascadeReasons.Count; $i++) {
            $r = $e.CascadeReasons[$i]
            if ($finalVersionByName.ContainsKey($r.Target) -and $r.Version -ne $finalVersionByName[$r.Target]) {
                $e.CascadeReasons[$i] = [pscustomobject]@{
                    Target   = $r.Target
                    Version  = $finalVersionByName[$r.Target]
                    Breaking = $r.Breaking
                }
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
        [string]$packageCargoToml,
        [string]$rootCargoToml
    )

    if ([string]::IsNullOrEmpty($version)) {
        Write-Error 'Update-PackageVersion: -version is required.' -ErrorAction Stop
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
    $packageContent = $script:CargoPackageVersionRegex.Replace($packageContent, ('${1}' + $version), 1)
    Set-Content -LiteralPath $packageCargoToml -Value $packageContent -NoNewline -Encoding utf8

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
    # Anchor the lookbehind to the start of a line (multiline mode) so the
    # package name cannot match as a suffix of another crate's name. Without
    # `^`, releasing e.g. `bar` would also rewrite `foo_bar = { ..., version
    # = "..." }` because the regex engine can satisfy the lookbehind by
    # matching `bar` against the trailing 3 chars of `foo_bar`. Workspace
    # dependency declarations in the root Cargo.toml are conventionally one
    # per line and flush-left, matching the layout produced by the test
    # fixture's `Write-RootCargoToml`.
    $regex = '(?m)(?<=^' + $packageNamePattern + '\s*=\s*\{[^\}]*?version\s*=\s*")[^"]+'
    (Get-Content -LiteralPath $rootCargoToml -Raw) -replace $regex, $version | Set-Content -LiteralPath $rootCargoToml -NoNewline -Encoding utf8

    return $version
}


# Locates a `## Unreleased` (or `## [Unreleased]`, case-insensitive) Markdown
# section in a changelog string and extracts its body lines, returning content
# with the section removed. Used by Write-Changelog to fold a manually-curated
# Unreleased section's contents into the new version section being created.
#
# Inputs:
#   -Content : full changelog text (the kind returned by Get-Content -Raw).
#
# Returns:
#   $null if no Unreleased section is present.
#   Otherwise a [pscustomobject] with:
#     BodyLines             - string[] (always an array; possibly empty) — the
#                             section's body split into lines, with leading
#                             and trailing blank lines stripped. Internal blank
#                             lines are preserved.
#     ContentWithoutSection - string — original content with the matched
#                             section (header + body) removed.
#
# Match semantics:
#   - Header line matches `## Unreleased` or `## [Unreleased]`, with optional
#     trailing whitespace. Case-insensitive on the word `Unreleased`.
#   - Body spans from the line after the header up to (but not including) the
#     next `## ` line at column 0, or end-of-input.
#   - Only the FIRST Unreleased section is extracted (it is unconventional for
#     a changelog to contain more than one).
function Extract-UnreleasedSection {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Content
    )

    if ([string]::IsNullOrEmpty($Content)) {
        return $null
    }

    # (?ims) — Multiline (^ matches line starts) + Singleline (. matches
    # newlines, so the non-greedy body can span lines) + IgnoreCase.
    $pattern = '(?ims)^##[ \t]+(?:\[Unreleased\]|Unreleased)[ \t]*\r?\n(?<body>.*?)(?=^##[ \t]|\z)'
    $match = [regex]::Match($Content, $pattern)
    if (-not $match.Success) {
        return $null
    }

    $body = $match.Groups['body'].Value
    $lines = @($body -split "`r?`n")

    # Strip trailing blank lines.
    while ($lines.Count -gt 0 -and [string]::IsNullOrWhiteSpace($lines[-1])) {
        $lines = if ($lines.Count -eq 1) { @() } else { @($lines[0..($lines.Count - 2)]) }
    }
    # Strip leading blank lines.
    while ($lines.Count -gt 0 -and [string]::IsNullOrWhiteSpace($lines[0])) {
        $lines = if ($lines.Count -eq 1) { @() } else { @($lines[1..($lines.Count - 1)]) }
    }

    return [pscustomobject]@{
        BodyLines             = [string[]]$lines
        ContentWithoutSection = $Content.Remove($match.Index, $match.Length)
    }
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
        # .Breaking) so both hashtables and [pscustomobject] are accepted.
        [object[]]$cascadeReasons = $null
    )

    $hasCascade = ($null -ne $cascadeReasons) -and ($cascadeReasons.Count -gt 0)

    # Read the existing changelog up front and extract any `## Unreleased`
    # section. The body of that section will be folded into the new version
    # section we're about to create — leaving it behind would orphan
    # manually-curated release notes below the freshly-inserted version
    # heading. Unreleased presence alone is enough reason to write a new
    # section, so we check it in the no-content guard below.
    $existingContent          = $null
    $existingHadContent       = $false
    $unreleasedLines          = @()
    if (Test-Path $changelogFile) {
        $existingContent = Get-Content $changelogFile -Raw
        if ($existingContent) {
            $existingHadContent = $true
            $extracted = Extract-UnreleasedSection -Content $existingContent
            if ($null -ne $extracted) {
                $unreleasedLines  = $extracted.BodyLines
                $existingContent  = $extracted.ContentWithoutSection
            }
        }
    }

    $hasUnreleased = $unreleasedLines.Count -gt 0

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

    # Get commits since the latest tag (unreleased commits) that touched this package, together
    # with the per-commit list of files each one changed within the folder. A single `git log
    # --name-only` does this in one process invocation (rather than a `git show` per commit), so
    # releases with large ranges stay fast. Each commit record is prefixed with a record separator
    # (0x1e); within the header the hash and subject are split by a unit separator (0x1f); the
    # file list then follows on subsequent lines (emitted by --name-only).
    $range = if ($latestTag) { "$latestTag..HEAD" } else { "HEAD" }
    $recordSep = [char]0x1e
    $unitSep = [char]0x1f
    $logOutput = Invoke-Git -Arguments @('log', $range, '--name-only', "--pretty=format:${recordSep}%H${unitSep}%s", '--', $packageFolder)
    $logText = @($logOutput) -join "`n"

    # Files in a package folder that are produced or maintained automatically rather than by a
    # human authoring a change. README.md is regenerated by `just readme`, which rewrites every
    # crate's README workspace-wide; an unrelated commit (e.g. one that introduced a *different*
    # package) therefore "touches" this folder solely through that regeneration. CHANGELOG.md is
    # release bookkeeping. A commit whose ONLY changes within this folder are such files did not
    # actually change the package, so its subject must not leak into the changelog.
    #
    # Only the crate-ROOT README.md / CHANGELOG.md are auto-maintained. Nested files are
    # hand-authored and must still count as meaningful changes — including ones that merely share
    # the leaf name (crates/<pkg>/examples/README.md) AND ones that also share the parent leaf
    # (crates/<pkg>/<pkg>/README.md). Matching on the parent leaf alone misclassifies the latter,
    # so we build the exact repo-relative crate-root paths and require a full-path match.
    #
    # git emits repo-relative, forward-slash paths regardless of platform. The package's
    # repo-relative directory is its last two path segments (crates/<pkg>); deriving it this way
    # avoids hard-coding the 'crates' prefix while still anchoring the match to the crate root.
    $packageSegments = $packageFolder -split '[\\/]' | Where-Object { $_ }
    $packageRelDir = ($packageSegments | Select-Object -Last 2) -join '/'
    $autoMaintainedRootPaths = @('README.md', 'CHANGELOG.md') | ForEach-Object { "$packageRelDir/$_" }

    $rawCommits = @()
    foreach ($record in ($logText -split $recordSep)) {
        if ([string]::IsNullOrWhiteSpace($record)) { continue }

        $header, $fileBlock = $record -split "`n", 2
        $subject = ($header -split $unitSep, 2)[-1].Trim()
        $changedFiles = @(($fileBlock -split "`n") | ForEach-Object { $_.Trim() } | Where-Object { $_ })

        $hasMeaningfulChange = $false
        foreach ($file in $changedFiles) {
            if ($autoMaintainedRootPaths -notcontains $file) {
                $hasMeaningfulChange = $true
                break
            }
        }
        if ($hasMeaningfulChange) {
            $rawCommits += $subject
        }
    }

    $formattedCommits = @()
    if ($rawCommits.Count -gt 0) {
        $formattedCommits = Format-ConventionalCommits -rawCommitMessages $rawCommits -prBaseUrl $prBaseUrl
    }

    if ($formattedCommits.Count -eq 0 -and -not $hasCascade -and -not $hasUnreleased) {
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

    # Build the new version section. User-curated `## Unreleased` body lines
    # (if any) lead the section so the manually-authored narrative appears
    # first; cascade bullets + commit-derived bullets follow as supplementary
    # detail. A blank line separates the two groups when both are present.
    $newVersionSection = @("## [$newVersion] - $currentDate", "")
    if ($hasUnreleased) {
        $newVersionSection += $unreleasedLines
        if ($formattedCommits.Count -gt 0) {
            $newVersionSection += ""
        }
    }
    $newVersionSection += $formattedCommits
    $newVersionSection += ""

    # Insert the new version section into the existing changelog, using the
    # Unreleased-stripped content as the base (so the orphaned `## Unreleased`
    # heading is no longer present in the output).
    if ($existingHadContent) {
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


function Show-ReleaseSummary {
    param(
        [array]$releases
    )

    Write-Host ""
    Write-Host "📦 Released packages:" -ForegroundColor Green
    foreach ($r in @($releases | Sort-Object -Property Package)) {
        Write-Host "  - $($r.Package): $($r.OldVersion) -> $($r.NewVersion)" -ForegroundColor Green
    }
    Write-Host ""
}

function Test-InteractiveSession {
    if ($env:CI) { return $false }
    if ($env:GITHUB_ACTIONS) { return $false }
    try { if ([Console]::IsInputRedirected) { return $false } } catch { }
    return $true
}

# --- PER-PACKAGE MENU PROMPT FLOW ---
#
# Helpers backing Invoke-PlanReview's per-package menu. Split out so pure
# formatting can be unit-tested without capturing host streams, and so the
# diff / opener side-effects can be mocked individually.

# Tracks temp files produced by Show-PackageDiff so Invoke-PlanReview can
# delete them at the end of the run. The plan-review entrypoint save/restores
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

# Returns $true when option 4 (Release as non-breaking change) would be
# numerically indistinguishable from option 3 (Release as breaking change) for
# the given current version. Under Cargo's 0.0.x semver carve-out every change
# type (breaking, non-breaking, patch) collapses to the same 0.0.(x+1)
# increment, so there is no point offering the non-breaking option — and the
# user should be told that all releases at this version range are considered
# breaking changes by Cargo. Returns $false for 0.x.y (y >= 1) where breaking
# (0.(y+1).0) still differs from non-breaking (0.y.(z+1)). When CurrentVersion
# is unknown we conservatively return $false so all options remain visible.
function Test-IsNonBreakingOptionRedundant {
    param([Parameter(Mandatory = $true)][AllowNull()][AllowEmptyString()][string]$CurrentVersion)

    if ([string]::IsNullOrWhiteSpace($CurrentVersion)) { return $false }
    $nonBreakingNext = Get-NextVersion -currentVersion $CurrentVersion -ChangeType 'non-breaking'
    $breakingNext = Get-NextVersion -currentVersion $CurrentVersion -ChangeType 'breaking'
    return ($nonBreakingNext -eq $breakingNext)
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
#
# Option 4 (Release as non-breaking change) is hidden when it would produce
# the same numeric increment as option 3 (Release as breaking change) — see
# Test-IsNonBreakingOptionRedundant. This is the case for 0.0.x packages,
# where Cargo treats every version bump as breaking; we append a one-line
# hint explaining why so the user is not left wondering what happened to the
# missing options.
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

    $hideNonBreaking = Test-IsNonBreakingOptionRedundant -CurrentVersion $current
    $hidePatch = $hideNonBreaking -or (Test-IsPatchOptionRedundant -CurrentVersion $current)

    # ChangedFileCount may be 0 (or missing) in -All mode where the planner
    # surfaces every published package regardless of on-disk modification
    # status. Adapt the header verb and the "View diff" label so the reviewer
    # is not misled into expecting changes that aren't there. The "View diff"
    # menu option stays in slot 1 in both cases so muscle memory works.
    $changeCount = 0
    if ($null -ne $Finding.PSObject.Properties['ChangedFileCount']) {
        $rawCount = $Finding.ChangedFileCount
        if ($null -ne $rawCount) {
            $changeCount = [int]$rawCount
        }
    }
    $hasChanges = $changeCount -gt 0

    $headerLine = if ($hasChanges) {
        "Detected package with unreleased modifications: $folder$queueSuffix"
    } else {
        "Reviewing package (no detected changes): $folder$queueSuffix"
    }
    $viewDiffLabel = if ($hasChanges) { 'View diff' } else { 'View diff (no changes in this package)' }

    $sb = [System.Text.StringBuilder]::new()
    [void]$sb.AppendLine('')
    [void]$sb.AppendLine($headerLine)
    # Show direct in-workspace dependents (packages that import this one
    # directly) as a comma-separated list. We deliberately omit transitive
    # dependents and full chains: in a workspace with hundreds of packages
    # the multi-line chain printout was overwhelming and rarely told the
    # reviewer anything they could act on. Direct dependents are what
    # cascade-toward-dependents pivots on, so they remain the most relevant
    # signal at decision time. Derived from WorkspaceDependencyChains so the
    # set is stable and release-set-independent (see chain construction in
    # Get-InWorkspaceDependencyChains: each chain is [root, ..., target],
    # so the direct dependent of the target is the second-to-last element).
    $chains = @($Finding.WorkspaceDependencyChains)
    $directDependents = [System.Collections.Generic.SortedSet[string]]::new(
        [System.StringComparer]::Ordinal)
    foreach ($chain in $chains) {
        $arr = @($chain)
        if ($arr.Length -lt 2) { continue }
        [void]$directDependents.Add($arr[$arr.Length - 2])
    }
    if ($directDependents.Count -gt 0) {
        $dependentLabel = if ($directDependents.Count -eq 1) { 'Direct dependent' } else { 'Direct dependents' }
        [void]$sb.AppendLine("  $($dependentLabel) in this workspace: $($directDependents -join ', ')")
    } else {
        [void]$sb.AppendLine('  No in-workspace dependents')
    }
    [void]$sb.AppendLine('')
    [void]$sb.AppendLine("  1. $viewDiffLabel")
    [void]$sb.AppendLine('  2. No material changes - release only if required by cascade logic')
    [void]$sb.AppendLine("  3. Release as breaking change $($changeTypeHints['breaking'])")
    if (-not $hideNonBreaking) {
        [void]$sb.AppendLine("  4. Release as non-breaking change $($changeTypeHints['non-breaking'])")
    }
    if (-not $hidePatch) {
        [void]$sb.AppendLine("  5. Release as patch $($changeTypeHints['patch'])")
    }
    if ($hideNonBreaking) {
        [void]$sb.AppendLine('')
        # Single-quoted to preserve the literal backticks around `0.0.` verbatim
        # (backticks are PowerShell's escape character in double-quoted strings).
        [void]$sb.AppendLine('  Note: all releases are considered breaking changes for package versions starting with `0.0.`')
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
# $script:TempPackageDiffPaths so Invoke-PlanReview can clean up.
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
# [1-4] and "5" is treated as an invalid choice. When option 4 is also
# suppressed (because on 0.0.x packages every release is breaking — see
# Test-IsNonBreakingOptionRedundant), the prompt range tightens to [1-3]
# and both "4" and "5" are treated as invalid. This keeps the prompt
# honest with what the menu shows.
function Get-PackageReleaseDecision {
    param(
        [Parameter(Mandatory = $true)][object]$Finding,
        [Parameter(Mandatory = $true)][int]$RemainingCount,
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    $current = [string]$Finding.CurrentVersion
    $hideNonBreaking = Test-IsNonBreakingOptionRedundant -CurrentVersion $current
    $hidePatch = $hideNonBreaking -or (Test-IsPatchOptionRedundant -CurrentVersion $current)
    $maxChoice = if ($hideNonBreaking) { 3 } elseif ($hidePatch) { 4 } else { 5 }

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
        if ($choice -eq '4' -and -not $hideNonBreaking) { return @{ Action = 'non-breaking' } }
        if ($choice -eq '5' -and -not $hidePatch) { return @{ Action = 'patch' } }

        Write-Host "Invalid choice '$choice'. Enter a number from 1 to $maxChoice." -ForegroundColor Yellow
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

# --- BUNDLED-INPUT RELEASE FLOW ---
#
# The bundled-input flow takes the entire release plan up front (via
# release-packages.ps1's -Packages parameter). The user reviews and decides
# every release in one transaction; the script then writes everything to
# disk atomically. This replaces the iterative single-package model where
# the user had to call the script repeatedly and reconcile on-disk state
# across invocations.
#
# Top-level shape:
#
#   Parse-ReleaseTokens          (-Packages -> parsed token objects)
#   |
#   v
#   Workspace baseline snapshot  (Get-WorkspacePackages, immutable for the run)
#   Modified-on-disk snapshot    (Get-PackagesWithUnreleasedChanges, immutable)
#   |
#   v
#   Invoke-PlanReview            (interactive elevation loop — pure planning,
#                                 no disk writes; loops until findings are
#                                 empty or all reviewed; produces final
#                                 ResolvedReleaseSet hashtable)
#   |
#   v
#   Show-ReleasePlan             (display the final plan to the user)
#   |
#   v
#   Invoke-ResolvedRelease       (execute the plan in topo order — writes
#                                 Cargo.toml / CHANGELOG.md / README.md for
#                                 every release-set member; produces release
#                                 records for the summary)
#   |
#   v
#   Show-ReleaseSummary + Show-FinalMessageForBundle
#

# Pre-release interactive elevation review loop. Operates entirely on in-memory
# state (a working list of parse-tokens, a $declined hashset of NON-release-set
# folders the user said "no" to, and a $reviewedCascadeAsIs hashset of
# release-set cascade-source folders the user explicitly said "this cascade
# change type is fine, don't elevate"). On each loop:
#
#   1. Re-resolve the release set from the current $userTokens via
#      Resolve-ReleaseSet (cheap — operates on the immutable workspace
#      baseline, no I/O). In -Mode 'all-changed' with no user tokens yet
#      this resolves to an empty set without invoking Resolve-ReleaseSet
#      (which throws on empty input).
#   2. Compute findings via Get-UnreleasedModifiedDependencies against the
#      fresh release set + immutable modifications snapshot. In -Mode
#      'all-changed' the call passes -IncludeAllModifiedAsRoots so every
#      changed published package surfaces from iteration 1. Filter out
#      $declined (still-not-in-release-set folders the user declined) and
#      $reviewedCascadeAsIs (release-set cascade-source folders the user
#      already accepted as-is).
#   3. If empty: review complete — return the resolved release set.
#   4. Prompt the user for the first finding via Get-PackageReleaseDecision.
#      On 'ignore' add to the appropriate hashset; on accept
#      ('breaking'/'non-breaking'/'patch') append a synthetic
#      '<folder>@<change>' token to $userTokens and loop. The menu's
#      "view diff" option is owned by Get-PackageReleaseDecision and never
#      returns control here.
#
# Interactivity is enforced by the entry-point (Invoke-ReleasePackagesMain);
# this function assumes a terminal is available for Read-Host.
#
# Decisions are FINAL. If a previously-declined package is later cascade-pulled
# into the release set, or a previously-reviewed-as-is package has its cascade
# level strengthened by a subsequent acceptance, the user is NOT re-prompted.
# Their "ignore" decision is interpreted as "accept whatever cascade level the
# planner decides; don't bother me about elevation" — a preference invariant
# under cascade-level changes. Cascade reasons for each released package are
# surfaced by Show-ReleasePlan's output for transparency.
#
# Returns: hashtable (folder -> resolved entry) representing the final plan.
#
# Termination: each iteration must change state (adds to $userTokens via
# accept, OR to $declined / $reviewedCascadeAsIs via ignore). Verified by a
# state-signature comparison at the top of each iteration — if two consecutive
# iterations produce the same signature we throw a "no progress" diagnostic
# rather than infinite-loop. A soft runaway cap (10 * published-package count)
# bounds total prompts as a defence-in-depth safety net; the real bound is
# one prompt per published package (the first time it surfaces).
function Invoke-PlanReview {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$ParsedTokens,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$WorkspaceBaseline,
        [Parameter(Mandatory = $false)][hashtable]$ModifiedSnapshot,
        [Parameter(Mandatory = $false)][ValidateSet('targeted', 'all-changed')][string]$Mode = 'targeted',
        [Parameter(Mandatory = $false)][switch]$Force
    )

    # Interactivity is enforced by the entry-point Invoke-ReleasePackagesMain.
    # By the time we get here we know we can Read-Host without failing.

    # Working token list, mutable. Each accepted finding appends a new token.
    $userTokens = New-Object 'System.Collections.Generic.List[object]'
    foreach ($t in $ParsedTokens) { $userTokens.Add($t) }

    $declined = [System.Collections.Generic.HashSet[string]]::new()
    # Set of release-set cascade-source folders the user said "keep cascade-
    # applied level, don't elevate". Entries are never removed: the decision
    # stands even if cascade strengthens the level on a later iteration.
    $reviewedCascadeAsIs = [System.Collections.Generic.HashSet[string]]::new()

    # Runaway cap is a defence-in-depth safety net; the real termination
    # guarantee comes from the state-signature progress check below. Each
    # published package is reviewed at most once (decisions are final), so
    # 10x the published count is comfortably above the worst case.
    $publishedCount = @(Get-WorkspacePackages -repoRoot $RepoRoot | Where-Object { $_.Published }).Count
    if ($publishedCount -lt 1) { $publishedCount = 1 }
    $runawayCap = 10 * $publishedCount

    # Save/restore the temp-diff-paths tracking list (used by Show-PackageDiff)
    # to match the lifecycle of this review loop.
    $prevTempPaths = $script:TempPackageDiffPaths
    $script:TempPackageDiffPaths = [System.Collections.Generic.List[string]]::new()

    $resolvedHash = $null
    $previousSignature = $null

    try {
        for ($iter = 0; $iter -lt $runawayCap; $iter++) {
            # State signature: every iteration must mutate at least one of
            # {userTokens, declined, reviewedCascadeAsIs}. The current control
            # flow guarantees this — accept appends to userTokens; ignore adds
            # to declined or reviewedCascadeAsIs; the switch's default arm
            # throws on any unrecognised action; an empty queue early-returns.
            # The signature check is therefore unreachable in normal
            # operation, but kept as defense-in-depth so a future change that
            # introduces a state-leak path (e.g. a new `continue`-without-
            # mutate branch) aborts with a clear diagnostic instead of
            # silently spinning until the runaway cap fires.
            $tokenSig    = (@($userTokens.ToArray()) | ForEach-Object { $_.RawToken }) -join '|'
            $declinedSig = (@($declined) | Sort-Object) -join ','
            $reviewedSig = (@($reviewedCascadeAsIs) | Sort-Object) -join ','
            $signature   = "tokens=[$tokenSig];declined=[$declinedSig];reviewed=[$reviewedSig]"
            if ($iter -gt 0 -and $signature -eq $previousSignature) {
                throw "Plan review made no progress on iteration $iter (state signature unchanged). This indicates a logic bug; please report. Signature: $signature"
            }
            $previousSignature = $signature

            # Re-resolve the release set from the current token list. Pure
            # in-memory operation; no caching/snapshot invalidation needed.
            # In all-changed mode the user may have accepted nothing yet, in
            # which case Resolve-ReleaseSet throws on empty input — handle
            # that here rather than relaxing the earlier guard, which would
            # weaken targeted-mode validation.
            if ($Mode -eq 'all-changed' -and $userTokens.Count -eq 0) {
                $resolvedHash = @{}
            } else {
                $resolvedArr  = @(Resolve-ReleaseSet -ParsedTokens $userTokens.ToArray() -WorkspaceBaseline $WorkspaceBaseline -Force:$Force)
                $resolvedHash = @{}
                foreach ($e in $resolvedArr) { $resolvedHash[$e.Folder] = $e }
            }

            # Handoff: a previously-declined or previously-reviewed-as-is folder
            # may now have a different cascade story (cascade pulled it into the
            # release set, or strengthened its level). Decisions are final, so we
            # do NOT re-prompt — the user's earlier "ignore" stands and the
            # cascade-applied level is silently accepted. Show-ReleasePlan's
            # output records the cascade reasons for transparency.

            Write-Host ''
            Write-Host '🔍 Analyzing packages for unreleased modifications...' -ForegroundColor Cyan

            if ($Mode -eq 'all-changed') {
                $allFindings = @(Get-UnreleasedModifiedDependencies -RepoRoot $RepoRoot -ResolvedReleaseSet $resolvedHash -ModifiedSnapshot $ModifiedSnapshot -IncludeAllModifiedAsRoots)
            } else {
                $allFindings = @(Get-UnreleasedModifiedDependencies -RepoRoot $RepoRoot -ResolvedReleaseSet $resolvedHash -ModifiedSnapshot $ModifiedSnapshot)
            }

            $queue = @(
                $allFindings | Where-Object {
                    -not $declined.Contains($_.Folder) -and
                    -not $reviewedCascadeAsIs.Contains($_.Folder)
                }
            )

            if ($queue.Count -eq 0) {
                Write-Host ''
                Write-Host '✅ No further unreleased modifications detected; release plan finalised.' -ForegroundColor Green
                return $resolvedHash
            }

            $next      = $queue[0]
            $remaining = $queue.Count - 1
            $decision  = Get-PackageReleaseDecision -Finding $next -RemainingCount $remaining -RepoRoot $RepoRoot

            if ($decision.Action -eq 'ignore') {
                if ($next.InReleaseSet) {
                    $cascadeLevel = $resolvedHash[$next.Folder].EffectiveChangeType
                    Write-Host "  Keeping '$($next.Folder)' at its cascade-applied $cascadeLevel level; reviewer should confirm no further elevation is needed." -ForegroundColor DarkGray
                    [void]$reviewedCascadeAsIs.Add($next.Folder)
                } else {
                    Write-Host "  Skipping '$($next.Folder)'; cascade may still pull it into the release plan on a later iteration." -ForegroundColor DarkGray
                    [void]$declined.Add($next.Folder)
                }
                continue
            }

            # Accept: synthesise a token. The decision action vocabulary
            # ('breaking'/'non-breaking'/'patch') maps to the parse-token
            # change-spec vocabulary ('breaking'/'nonbreaking'/'patch').
            #
            # For both new and elevation cases we craft the parsed-token object
            # directly rather than going through Parse-ReleaseTokens. That's
            # safe because Resolve-ReleaseSet's duplicate-folder check only
            # fires against other user-source tokens — and the only re-surfaced
            # findings are cascade-source entries (user-source members are never
            # re-surfaced, per Get-UnreleasedModifiedDependencies's predicate).
            $changeSpec = switch ($decision.Action) {
                'breaking'     { 'breaking' }
                'non-breaking' { 'nonbreaking' }
                'patch'        { 'patch' }
                default        { throw "Internal error: Get-PackageReleaseDecision returned unexpected action '$($decision.Action)'." }
            }
            $newToken = "$($next.Folder)@$changeSpec"
            $userTokens.Add(([pscustomobject]@{
                Name                   = $next.Folder
                RequestedChangeType    = $decision.Action
                RequestedTargetVersion = $null
                RawToken               = $newToken
            }))
        }

        Write-Warning "Plan review reached its runaway-cap of $runawayCap iterations; aborting further prompts. This is a defence-in-depth safety net — the state-signature check above should have caught any logic loop earlier; if you see this, please report."
        # Re-resolve before returning so the final acceptance of the last
        # iteration (if any) is reflected in the plan handed back to the
        # caller. Without this, callers see the resolved set from the START
        # of the final iteration, missing the token just appended.
        if ($Mode -eq 'all-changed' -and $userTokens.Count -eq 0) {
            $resolvedHash = @{}
        } else {
            $resolvedArr  = @(Resolve-ReleaseSet -ParsedTokens $userTokens.ToArray() -WorkspaceBaseline $WorkspaceBaseline -Force:$Force)
            $resolvedHash = @{}
            foreach ($e in $resolvedArr) { $resolvedHash[$e.Folder] = $e }
        }
        return $resolvedHash
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

# Topological sort of a resolved release set: dependencies first, dependents
# last. Uses Kahn's algorithm against the workspace baseline so the order is
# deterministic and unaffected by hashtable enumeration order.
#
# Folders with no in-set dependencies come first (the "leaves" of the
# release-set sub-DAG). Among equal-rank candidates, ties are broken by
# folder name so output is reproducible across runs.
function Get-TopoOrderedReleaseFolders {
    param(
        [Parameter(Mandatory = $true)][hashtable]$ResolvedReleaseSet,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$WorkspaceBaseline
    )

    if ($ResolvedReleaseSet.Count -eq 0) { return @() }

    $folders = @($ResolvedReleaseSet.Keys)
    $byFolder = @{}
    $byCargo  = @{}
    foreach ($pkg in $WorkspaceBaseline) {
        $byFolder[$pkg.Folder] = $pkg
        $byCargo[$pkg.Name.Replace('-', '_')] = $pkg
    }

    # Build adjacency: for each release-set folder, the in-set folders it
    # depends on (its deps that are also in the release set).
    $inSetDeps = @{}
    $inDegree  = @{}
    foreach ($folder in $folders) {
        $pkg = $byFolder[$folder]
        $deps = New-Object 'System.Collections.Generic.HashSet[string]'
        if ($null -ne $pkg) {
            foreach ($depCargo in $pkg.Deps) {
                $depPkg = $byCargo[$depCargo]
                if ($null -ne $depPkg -and $ResolvedReleaseSet.ContainsKey($depPkg.Folder)) {
                    [void]$deps.Add($depPkg.Folder)
                }
            }
        }
        $inSetDeps[$folder] = $deps
        $inDegree[$folder] = $deps.Count
    }

    $ready = [System.Collections.Generic.List[string]]::new()
    foreach ($f in $folders) {
        if ($inDegree[$f] -eq 0) { $ready.Add($f) }
    }

    $result = [System.Collections.Generic.List[string]]::new()
    while ($ready.Count -gt 0) {
        $sortedReady = @($ready | Sort-Object)
        $next = $sortedReady[0]
        [void]$ready.Remove($next)
        $result.Add($next)

        foreach ($f in $folders) {
            if ($inSetDeps[$f].Contains($next)) {
                $inDegree[$f] = $inDegree[$f] - 1
                if ($inDegree[$f] -eq 0) { $ready.Add($f) }
            }
        }
    }

    if ($result.Count -ne $folders.Count) {
        # Cycle in dependencies among release-set members — the workspace
        # itself would already be broken; surface it loudly.
        throw "Get-TopoOrderedReleaseFolders: dependency cycle detected among release-set members; cannot determine release order."
    }

    return $result.ToArray()
}

# Executes a finalised release plan. For each release-set entry, in topo order
# (dependencies first), writes Cargo.toml + workspace Cargo.toml + CHANGELOG +
# README. No cascade logic, no user prompts — every release decision was
# already made in Invoke-PlanReview.
#
# Returns release records: @(@{Package; OldVersion; NewVersion}, ...) in
# release order.
#
# The function is plan-driven: it never re-reads the on-disk Cargo.toml to
# determine the next version. The plan's EffectiveTargetVersion is the source
# of truth. This makes Invoke-ResolvedRelease provably independent of any
# mid-execution disk state observation.
function Invoke-ResolvedRelease {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$RootCargoToml,
        [Parameter(Mandatory = $false)][string]$PrBaseUrl,
        [Parameter(Mandatory = $true)][hashtable]$ResolvedReleaseSet,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$WorkspaceBaseline
    )

    if ($ResolvedReleaseSet.Count -eq 0) { return @() }

    $orderedFolders = Get-TopoOrderedReleaseFolders -ResolvedReleaseSet $ResolvedReleaseSet -WorkspaceBaseline $WorkspaceBaseline

    $records = New-Object 'System.Collections.Generic.List[object]'

    foreach ($folder in $orderedFolders) {
        $entry            = $ResolvedReleaseSet[$folder]
        $packageFolder    = Join-Path $RepoRoot 'crates' $folder
        $packageCargoToml = Join-Path $packageFolder 'Cargo.toml'
        $changelogFile    = Join-Path $packageFolder 'CHANGELOG.md'

        $oldVersion = $entry.CurrentVersion
        $newVersion = $entry.EffectiveTargetVersion

        Write-Host ''
        $sourceLabel = if ($entry.Source -eq 'user') { 'user-requested' } else { 'cascade-from-dependency' }
        Write-Host "🚀 Releasing '$folder' ($sourceLabel): $oldVersion -> $newVersion" -ForegroundColor Cyan

        # The plan's EffectiveTargetVersion is taken verbatim — this keeps
        # the executor plan-driven.
        $written = Update-PackageVersion -packageName $entry.Name -version $newVersion `
            -packageCargoToml $packageCargoToml -rootCargoToml $RootCargoToml
        if ($null -eq $written) {
            Write-Error "Failed to update version for package '$folder'." -ErrorAction Stop
        }

        $cascadeReasons = if ($null -ne $entry.CascadeReasons -and $entry.CascadeReasons.Count -gt 0) {
            # .ToArray() instead of @(...) — PowerShell's array sub-expression
            # operator can't iterate a List[object] held in a property accessor.
            $entry.CascadeReasons.ToArray()
        } else {
            $null
        }
        Write-Changelog -packageName $entry.Name -newVersion $newVersion -packageFolder $packageFolder `
            -changelogFile $changelogFile -prBaseUrl $PrBaseUrl -cascadeReasons $cascadeReasons

        Update-Readme -packageName $entry.Name -packageFolder $packageFolder

        $records.Add([pscustomobject]@{
            Package    = $folder
            OldVersion = $oldVersion
            NewVersion = $newVersion
        })
    }

    # The on-disk workspace metadata is now stale (we just rewrote Cargo.tomls);
    # later operations that rely on cargo metadata (e.g. Invoke-WorkspaceCheck)
    # must observe the new state.
    Invalidate-WorkspaceMetadataCache

    return $records.ToArray()
}

# Pretty-prints the resolved release plan before execution so the user can
# eyeball the final state. Lists user-source members first (in token order),
# then cascade-source members (sorted by folder for determinism). For each
# entry, shows the version transition, the source, the effective change
# type, and any cascade reasons. AutoUpgraded user-source entries are
# flagged so the user notices when cascade strengthened their request.
function Show-ReleasePlan {
    param(
        [Parameter(Mandatory = $true)][hashtable]$ResolvedReleaseSet
    )

    if ($ResolvedReleaseSet.Count -eq 0) {
        Write-Host ''
        Write-Host '📋 Release plan: (empty)' -ForegroundColor Yellow
        return
    }

    $userEntries    = @($ResolvedReleaseSet.Values | Where-Object { $_.Source -eq 'user' })
    $cascadeEntries = @($ResolvedReleaseSet.Values | Where-Object { $_.Source -eq 'cascade' } | Sort-Object -Property Folder)

    $total = $ResolvedReleaseSet.Count
    $packageNoun = if ($total -eq 1) { 'package' } else { 'packages' }

    Write-Host ''
    Write-Host "📋 Final release plan ($total $packageNoun):" -ForegroundColor Cyan

    foreach ($entry in $userEntries) {
        $tag = if ($entry.PinHonoredAgainstCascade) {
            "user-requested ($($entry.EffectiveChangeType); -Force: pin honored over cascade)"
        } elseif ($entry.AutoUpgraded) {
            "user-requested (auto-upgraded by cascade to $($entry.EffectiveChangeType))"
        } else {
            "user-requested ($($entry.EffectiveChangeType))"
        }
        $color = if ($entry.PinHonoredAgainstCascade) { 'Yellow' } else { 'Green' }
        Write-Host "  • $($entry.Folder): $($entry.CurrentVersion) -> $($entry.EffectiveTargetVersion)   [$tag]" -ForegroundColor $color
        if ($null -ne $entry.CascadeReasons -and $entry.CascadeReasons.Count -gt 0) {
            $names = ($entry.CascadeReasons | ForEach-Object { $_.Target } | Sort-Object -Unique) -join ', '
            $reasonLabel = if ($entry.PinHonoredAgainstCascade) { 'cascade required upgrade from' } else { 'strengthened by cascade from' }
            Write-Host "      $($reasonLabel): $names" -ForegroundColor DarkGray
        }
    }

    foreach ($entry in $cascadeEntries) {
        Write-Host "  • $($entry.Folder): $($entry.CurrentVersion) -> $($entry.EffectiveTargetVersion)   [cascade ($($entry.EffectiveChangeType))]" -ForegroundColor DarkCyan
        $names = ($entry.CascadeReasons | ForEach-Object { $_.Target } | Sort-Object -Unique) -join ', '
        Write-Host "      cascaded from: $names" -ForegroundColor DarkGray
    }

    # Footer: explain how cascade interacts with user input. Always printed so
    # reviewers don't have to remember the contract; the conditional lines
    # only fire when the corresponding situation applies (at least one user
    # entry was auto-upgraded, or at least one was pinned over cascade via
    # -Force).
    Write-Host ''
    Write-Host 'Note: user-provided change types may be automatically upgraded if cascade logic deems it necessary (e.g. non-breaking -> breaking).' -ForegroundColor DarkGray
    $autoUpgraded = @($userEntries | Where-Object { $_.AutoUpgraded })
    if ($autoUpgraded.Count -gt 0) {
        $autoUpgradedLine = if ($autoUpgraded.Count -eq 1) {
            "Item above tagged 'auto-upgraded by cascade' was upgraded from the user-requested change type."
        } else {
            "Items above tagged 'auto-upgraded by cascade' were upgraded from the user-requested change type."
        }
        Write-Host $autoUpgradedLine -ForegroundColor DarkGray
    }
    $forcedPins = @($userEntries | Where-Object { $_.PinHonoredAgainstCascade })
    if ($forcedPins.Count -gt 0) {
        $forcedPinsLine = if ($forcedPins.Count -eq 1) {
            "Item above tagged '-Force: pin honored over cascade' kept its explicit version pin even though cascade required a higher version — consumers may break."
        } else {
            "Items above tagged '-Force: pin honored over cascade' kept their explicit version pin even though cascade required a higher version — consumers may break."
        }
        Write-Host $forcedPinsLine -ForegroundColor Yellow
    }
    Write-Host 'If an explicit version number is specified in the release-spec but cascade logic requires a higher version number, the release plan is rejected (use -Force to override).' -ForegroundColor DarkGray
}

# Prints the "Success! Next steps" block after a bundled release. Picks the
# alphabetically-first user-source folder as the "primary" for the
# conventional-commits scope (best-effort heuristic; the multi-package wording
# supersedes it when more than one package is released).
function Show-FinalMessageForBundle {
    param(
        [Parameter(Mandatory = $true)][array]$Releases,
        [Parameter(Mandatory = $true)][hashtable]$ResolvedReleaseSet
    )

    if ($Releases.Count -eq 0) {
        Write-Host '---' -ForegroundColor Green
        Write-Host 'ℹ️  No releases produced; nothing to commit.' -ForegroundColor Green
        Write-Host '---' -ForegroundColor Green
        return
    }

    # Identify the primary by taking the first user-source folder in the plan
    # (alphabetic order; matches the topo-sort tie-breaker for stability).
    $userFolders = @($ResolvedReleaseSet.Values | Where-Object { $_.Source -eq 'user' } | ForEach-Object { $_.Folder } | Sort-Object)
    $primaryFolder = if ($userFolders.Count -gt 0) { $userFolders[0] } else { $Releases[0].Package }
    $primary = $Releases | Where-Object { $_.Package -eq $primaryFolder } | Select-Object -First 1
    if ($null -eq $primary) { $primary = $Releases[0] }

    $primaryName    = $primary.Package
    $primaryVersion = $primary.NewVersion

    $extraCount = @($Releases).Count - 1
    if ($extraCount -le 0) {
        $commitMessage = "feat($primaryName): release v$primaryVersion"
    } else {
        $extraNoun = if ($extraCount -eq 1) { 'additional package' } else { 'additional packages' }
        $commitMessage = "feat: release $primaryName v$primaryVersion and $extraCount $extraNoun"
    }

    Write-Host '---' -ForegroundColor Green
    Write-Host '🎉 Success! Next steps:' -ForegroundColor Green
    Write-Host '1. Review the changes in the updated files.' -ForegroundColor Green
    Write-Host '2. Commit the changes and push the changes:' -ForegroundColor Green
    Write-Host '   git add .' -ForegroundColor DarkGray
    Write-Host "   git commit -m `"$commitMessage`"" -ForegroundColor DarkGray
    Write-Host '   git push' -ForegroundColor DarkGray
    Write-Host '3. Once the commit is merged to main, automation will tag the commit and release to crates.io' -ForegroundColor Green
    Write-Host '---' -ForegroundColor Green
}

# Top-level entry point for the bundled-input release flow. Routes the three
# user-facing modes ('targeted' / 'changed' / 'all') through one shared
# pipeline: pre-flight checks, plan resolution / cascade / elevation review,
# plan display, atomic execution, post-execution workspace `cargo check`, and
# final summary message.
#
# Every mode is intended for interactive use — the elevation review prompts
# the user even in targeted mode if modified-but-unreleased dependencies of
# the requested packages are detected. The script refuses to run when stdin
# is not a terminal, regardless of mode.
#
# Targeted mode is the only mode that accepts a non-empty $Packages list.
# Changed and All modes discover their own targets:
#
#   - Changed surfaces every published workspace package with unreleased
#     modifications (changes newer than its last `version =` /
#     `publish =` commit).
#   - All surfaces every published workspace package, regardless of whether
#     the on-disk content has been modified. The change-detection scan still
#     runs (for ChangedFileCount accuracy in the menu) but its result is
#     augmented so the predicate inside Get-UnreleasedModifiedDependencies
#     accepts every published package as eligible.
#
# Returns the array of release records (so Pester scenarios can assert on
# them). Errors during input validation / pre-flight checks call Exit 1 to
# match the existing script CLI contract.
function Invoke-ReleasePackagesMain {
    [CmdletBinding()]
    param(
        [Parameter()]
        [ValidateSet('targeted', 'changed', 'all')]
        [string]$Mode = 'targeted',

        [Parameter()]
        [AllowNull()]
        [AllowEmptyCollection()]
        [string[]]$Packages = @(),

        [Parameter()]
        [switch]$Force
    )

    # 1. PRE-FLIGHT
    if (-not (Test-CommandExists -command 'git')) {
        Write-Error 'Git is not installed or not found in your PATH.'
        Exit 1
    }

    $repoRoot = Get-Location
    if (-not (Test-Path (Join-Path $repoRoot '.git'))) {
        Write-Error 'This script must be run from the root of a Git repository.'
        Exit 1
    }
    $rootCargoToml = Join-Path $repoRoot 'Cargo.toml'
    if (-not (Test-Path $rootCargoToml)) {
        Write-Error "Could not find root Cargo.toml at '$rootCargoToml'."
        Exit 1
    }

    # Every mode is interactive: the elevation review can prompt even in
    # targeted mode when a requested release has modified-but-unreleased
    # dependencies. Bail out early with a clear error if stdin is not a
    # terminal, rather than failing deep inside Read-Host.
    if (-not (Test-InteractiveSession)) {
        Write-Error 'release-packages.ps1 must be run from an interactive terminal — every mode may prompt the user for elevation review of modified-but-unreleased dependencies.'
        Exit 1
    }

    # 2. MODE / INPUT VALIDATION + TOKEN PARSE
    $hasTokens = ($null -ne $Packages) -and ($Packages.Count -gt 0)
    if ($Mode -ne 'targeted' -and $Force) {
        Write-Error "release-packages.ps1 -Force is only valid with -Packages (targeted mode). The -Changed and -All modes only accept change-type answers (breaking / non-breaking / patch) and never explicit version pins, so the pin-vs-cascade rejection that -Force overrides cannot fire."
        Exit 1
    }
    if ($Mode -eq 'targeted') {
        if (-not $hasTokens) {
            Write-Error 'release-packages.ps1 -Packages requires at least one ''<name>@<change-spec>'' token. Use -Changed or -All for a guided walk instead.'
            Exit 1
        }
        try {
            $parsedTokens = Parse-ReleaseTokens -Tokens $Packages
        } catch {
            Write-Error $_.Exception.Message
            Exit 1
        }
    } else {
        if ($hasTokens) {
            Write-Error "release-packages.ps1 -$Mode does not accept -Packages tokens; the planner discovers targets for you."
            Exit 1
        }
        $parsedTokens = @()
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

    # 4. SNAPSHOT WORKSPACE + MODIFICATIONS (immutable for the run)
    $workspaceBaseline = @(Get-WorkspacePackages -repoRoot $repoRoot.Path)
    $modifiedSnapshot  = Get-PackagesWithUnreleasedChanges -RepoRoot $repoRoot.Path

    # 4a. ALL-MODE SNAPSHOT AUGMENTATION (private to this entry-point).
    # Get-UnreleasedModifiedDependencies's surfacing predicate filters on
    # $modifiedMap.ContainsKey($folder). To make every publishable package
    # eligible without threading a new mode flag through the predicate, we
    # synthesise stub entries with ChangedFileCount = 0 for each published
    # package not already represented. The synthesised snapshot must NOT
    # escape this function — its zero-count entries would be misleading in
    # PR-comment / dep-scan contexts that consume "really modified" data.
    if ($Mode -eq 'all') {
        foreach ($pkg in $workspaceBaseline) {
            if ($pkg.Published -and -not $modifiedSnapshot.ContainsKey($pkg.Folder)) {
                $modifiedSnapshot[$pkg.Folder] = 0
            }
        }
    }

    # 4b. CHANGED-MODE EARLY EXIT — saves the user from an empty prompt loop.
    if ($Mode -eq 'changed' -and $modifiedSnapshot.Count -eq 0) {
        Write-Host ''
        Write-Host '✅ No workspace packages have unreleased modifications. Nothing to release.' -ForegroundColor Green
        return ,@()
    }

    # 5. PRE-RELEASE REVIEW (interactive loop; no disk writes).
    # 'changed' and 'all' both map to Invoke-PlanReview's 'all-changed' mode:
    # the planner adds every snapshot entry as a BFS root, surfacing them
    # one-by-one for a per-package decision. Acceptances become tokens inside
    # the loop and feed Resolve-ReleaseSet on the next iteration just like
    # the targeted flow.
    $planReviewMode = if ($Mode -eq 'targeted') { 'targeted' } else { 'all-changed' }
    try {
        $resolvedHash = Invoke-PlanReview -RepoRoot $repoRoot.Path `
            -ParsedTokens $parsedTokens -WorkspaceBaseline $workspaceBaseline `
            -ModifiedSnapshot $modifiedSnapshot -Mode $planReviewMode -Force:$Force
    } catch {
        Write-Error $_.Exception.Message
        Exit 1
    }

    # 6. EARLY EXIT IF GUIDED USER IGNORED EVERYTHING — skip Show-ReleasePlan
    # / Invoke-ResolvedRelease / Invoke-WorkspaceCheck. They handle empty
    # input gracefully but we'd waste a `cargo check` run and the user
    # already knows nothing will happen. Cannot apply to targeted mode
    # because targeted requires at least one token.
    if ($Mode -ne 'targeted' -and ($null -eq $resolvedHash -or $resolvedHash.Count -eq 0)) {
        Write-Host ''
        Write-Host '✅ No packages selected for release.' -ForegroundColor Green
        return ,@()
    }

    # 7. SHOW PLAN
    Show-ReleasePlan -ResolvedReleaseSet $resolvedHash

    # 8. EXECUTE PLAN (atomic — all writes happen here)
    try {
        $releases = @(Invoke-ResolvedRelease -RepoRoot $repoRoot.Path -RootCargoToml $rootCargoToml `
            -PrBaseUrl $prBaseUrl -ResolvedReleaseSet $resolvedHash -WorkspaceBaseline $workspaceBaseline)
    } catch {
        Write-Error "Release execution failed: $_"
        Exit 1
    }

    Invoke-WorkspaceCheck -RepoRoot $repoRoot.Path

    Show-ReleaseSummary -releases $releases
    Show-FinalMessageForBundle -Releases $releases -ResolvedReleaseSet $resolvedHash

    return ,$releases
}
