# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Shared helpers for package-release tooling. Dot-source from other scripts; never run directly.

.DESCRIPTION
    This file is a library, not an entrypoint. It is loaded into the caller's scope via
    dot-sourcing, e.g.

        . "$PSScriptRoot/lib/releasing.ps1"

    It exposes functions for:
      - Workspace metadata access (cached via `cargo metadata`).
      - Reverse-dependency cascade computation.
      - SemVer arithmetic (Cargo's 0.x.y rules).
      - Safe git invocation (no Invoke-Expression).
      - Detecting which packages have had their version incremented in this PR, which
        have had source modifications since their own last release baseline (per-package,
        derived from each package's Cargo.toml history), and which workspace dependencies of
        in-release packages fall into the "modified-but-unreleased" bucket (the core
        "unreleased workspace dependency" analysis).

    It has no top-level param() block and no side effects beyond declaring script-scope
    caches & compiled regexes.
#>

# --- COMPILED REGEX PATTERNS ---

$script:ConventionalCommitRegex = [regex]'^(\w+)(?:\(.*\))?(!)?:\s*(.*)'
$script:PrReferenceRegex = [regex]'\s*(\(#(\d+)\))$'
# Strict SemVer 2.0 grammar from https://semver.org/#is-there-a-suggested-regular-expression-regex-to-check-a-semver-string
# Anchored. Disallows leading zeros in numeric components AND in pre-release
# numeric identifiers. Allows optional pre-release (-...) and build (+...)
# suffixes. The [semver] PowerShell type would parse some illegal inputs (e.g.
# '01.2.3') so we validate with this regex first and only cast to [semver]
# afterwards for ordering operations.
$script:SemanticVersionRegex = [regex]'^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$'
# Matches a Cargo.toml's [package]-scoped `version = "..."` line.
#   - Anchored at line start so substring keys like `rust-version` do not match.
#   - Walks from the [package] header through lines that don't start a new TOML
#     table (`[...]`), so a `description = "Has [brackets]"` field above the
#     version line is fine but a `[package.metadata.*]` subtable interrupts the
#     match (we don't support a `[package]` block whose `version` lives after a
#     subtable — the version line is conventionally near the top).
#   - Group 1: prefix up to (and including) the opening quote.
#   - Group 2: the version literal itself.
$script:CargoPackageVersionRegex = [regex]'(?m)(^\[package\](?:\r?\n(?!\[)[^\n]*)*?\r?\n[ \t]*version[ \t]*=[ \t]*")([^"]+)'
$script:GitHubRepoRegex = [regex]'github\.com[/:]([\w.-]+/[\w.-]+)'
$script:RegexEscapeRegex = [regex]'([\\\.$\^\{\[\(\|\)\*\+\?\/])'

# --- SAFE GIT WRAPPER ---

# Runs `git` with the given positional argument array. Returns captured stdout as
# a string array (one element per line), or @() when there is no output. Throws on
# non-zero exit codes, with the command line and stderr included in the message.
# Uses explicit array arguments (no shell interpolation) so untrusted inputs
# (e.g. a -BaseRef value from CLI) cannot be shell-injected.
function Invoke-Git {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [string]$RepoRoot,
        [switch]$AllowFailure
    )

    $gitArgs = @()
    if ($RepoRoot) { $gitArgs += @('-C', $RepoRoot) }
    $gitArgs += $Arguments

    # Suppress strict native-command error handling locally; this function manages
    # exit codes manually via $LASTEXITCODE so callers (and AllowFailure) can react.
    $PSNativeCommandUseErrorActionPreference = $false
    $output = & git @gitArgs 2>&1
    $exitCode = $LASTEXITCODE

    if ($exitCode -ne 0) {
        if ($AllowFailure.IsPresent) {
            return $null
        }
        $cmdLine = "git $($gitArgs -join ' ')"
        $msg = if ($output) { ($output | Out-String).Trim() } else { '<no output>' }
        throw "Git command failed (exit $exitCode): $cmdLine`n$msg"
    }

    if ($null -eq $output) { return @() }
    return @($output)
}

# Returns $true if the given ref can be resolved locally, $false otherwise.
# Never throws.
function Test-GitRef {
    param(
        [Parameter(Mandatory = $true)][string]$Ref,
        [string]$RepoRoot
    )

    $null = Invoke-Git -Arguments @('rev-parse', '--verify', '-q', "$Ref^{commit}") -RepoRoot $RepoRoot -AllowFailure
    return ($LASTEXITCODE -eq 0)
}

# --- FILE I/O HELPERS ---

# Detects the dominant line-ending convention ("`r`n" or "`n") used by the
# file at -Path so callers can preserve it on write. Useful when the script
# is used across repos that may not all enforce LF line endings via
# .gitattributes. Returns "`n" when the file is missing, empty, or has no
# detectable line endings (the modern default).
function Get-FileLineEnding {
    param(
        [Parameter(Mandatory = $true)][string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) { return "`n" }
    $raw = Get-Content -LiteralPath $Path -Raw -Encoding utf8
    if ([string]::IsNullOrEmpty($raw)) { return "`n" }

    $crlf = ([regex]::Matches($raw, "`r`n")).Count
    # Count lone LFs (LFs not immediately preceded by CR) to avoid double-counting CRLF pairs.
    $lf   = ([regex]::Matches($raw, "(?<!`r)`n")).Count

    if ($crlf -gt $lf) { return "`r`n" }
    return "`n"
}

# --- VERSION HELPERS ---

function Test-ValidPackageName {
    param([string]$packageName)
    return $packageName -match '^[a-zA-Z0-9]([a-zA-Z0-9_-]*[a-zA-Z0-9])?$' -and $packageName.Length -le 64
}

function Test-ValidVersion {
    param([string]$version)
    if ([string]::IsNullOrEmpty($version)) {
        return $true
    }
    return $script:SemanticVersionRegex.IsMatch($version)
}

# Strict SemVer 2.0 splitter — validates with $script:SemanticVersionRegex and
# returns a hashtable with keys Major/Minor/Patch (all [int]) plus PreRelease
# and Build (strings, possibly empty). Throws on invalid input. Pre-release
# numeric identifiers are intentionally kept as strings since their grammar
# permits forms like '0' (allowed) but not '01' (rejected by the regex).
function Split-SemanticVersion {
    param([Parameter(Mandatory = $true)][string]$version)

    $m = $script:SemanticVersionRegex.Match($version)
    if (-not $m.Success) {
        throw "Invalid SemVer version '$version'. Expected the form <major>.<minor>.<patch>[-<prerelease>][+<build>] with exactly three numeric components (no leading zeros)."
    }

    return @{
        Major      = [int]$m.Groups[1].Value
        Minor      = [int]$m.Groups[2].Value
        Patch      = [int]$m.Groups[3].Value
        PreRelease = $m.Groups[4].Value
        Build      = $m.Groups[5].Value
    }
}

# Returns -1, 0, or 1 — SemVer 2.0 ordering (full Major/Minor/Patch +
# pre-release identifier comparison; build metadata is ignored per spec).
# Both inputs are validated strictly via Split-SemanticVersion and will throw
# on invalid input (including 1- or 2-component forms).
function Compare-SemanticVersions {
    param(
        [string]$version1,
        [string]$version2
    )

    # Validate via Split-SemanticVersion (throws on invalid input). [semver]
    # alone would silently accept '01.2.3' and similar non-canonical forms.
    [void](Split-SemanticVersion -version $version1)
    [void](Split-SemanticVersion -version $version2)

    $sv1 = [semver]$version1
    $sv2 = [semver]$version2
    if ($sv1 -gt $sv2) { return 1 }
    if ($sv1 -lt $sv2) { return -1 }
    return 0
}

# Computes the next version for the given change type, honoring Cargo's 0.x.y SemVer rules.
#
# IMPORTANT VOCABULARY (also documented in AGENTS.md "Release Versioning Vocabulary"):
#
#   * CHANGE TYPE — the semantic intent of a release: 'breaking' /
#     'non-breaking' / 'patch'. This is what the user thinks about; the change
#     type for each released package is supplied in the `-Packages` argument
#     to `release-packages.ps1` (e.g. `mypkg@breaking`, `mypkg@nonbreaking`).
#     Internally the same vocabulary is used for the `$changeType` enum (and
#     for `-ChangeType` parameters throughout the release tooling).
#
#   * VERSION COMPONENT — a position in the SemVer string `major.minor.patch`
#     (the integers in x.y.z). These names are POSITIONAL, not semantic.
#
# The mapping from change type to the actual version component that gets
# incremented depends on the current version:
#   - For x.y.z (x >= 1): breaking -> (x+1).0.0, non-breaking -> x.(y+1).0, patch -> x.y.(z+1)
#     (here the change type and the version-component name happen to coincide).
#   - For 0.x.y (x >= 1): breaking -> 0.(x+1).0 (the MINOR component is incremented!),
#                         non-breaking and patch -> 0.x.(y+1) (patch component).
#   - For 0.0.x          : every change -> 0.0.(x+1) (every change is breaking).
#
# DO NOT leak the internal `breaking|non-breaking|patch` enum directly into
# user-visible output without a translation step — use `Get-ChangeTypeLabel`
# in release-flow.ps1 to get a user-friendly noun phrase.
function Get-NextVersion {
    param(
        [string]$currentVersion,
        [ValidateSet('breaking', 'non-breaking', 'patch')]
        [string]$ChangeType
    )

    # Strict-parse the input. Pre-release / build suffixes are recognised but
    # dropped from the output — the next-version computation only operates on
    # the (major, minor, patch) triple, and we never emit pre-release versions
    # from a release (the release is always a clean SemVer).
    $parts = Split-SemanticVersion -version $currentVersion
    $major = $parts.Major
    $minor = $parts.Minor
    $patch = $parts.Patch

    if ($major -ge 1) {
        switch ($ChangeType) {
            'breaking'     { return "$($major + 1).0.0" }
            'non-breaking' { return "$major.$($minor + 1).0" }
            'patch'        { return "$major.$minor.$($patch + 1)" }
        }
    }
    elseif ($minor -ge 1) {
        switch ($ChangeType) {
            'breaking' { return "0.$($minor + 1).0" }
            default    { return "0.$minor.$($patch + 1)" }
        }
    }
    else {
        return "0.0.$($patch + 1)"
    }
}

# Recovers the change type implied by a (oldVersion -> newVersion) transition.
#
# NOTE: this function returns the CONSERVATIVE LOWER BOUND of the change type
# implied by the numeric transition. For a 0.x.y package the transition
# 0.4.1 -> 0.4.2 could have originated from EITHER a 'non-breaking' OR a
# 'patch' change type — both collapse to the same numeric increment under
# Cargo's 0.x SemVer rules. We return 'patch' in that case because that is the
# tightest claim we can make from numbers alone. Every consumer (cascade math,
# Test-IsBreakingChange) treats 'non-breaking' and 'patch' identically on 0.x
# packages, so the ambiguity has no functional impact at call sites.
function Get-ChangeTypeFromVersions {
    param(
        [string]$oldVersion,
        [string]$newVersion
    )

    # Strict-parse both inputs. Pre-release / build metadata is dropped from the
    # numeric-component comparison (pre-release-only transitions like
    # 1.0.0-pre01 → 1.0.0 are classified as the weakest 'patch').
    $oldParts = Split-SemanticVersion -version $oldVersion
    $newParts = Split-SemanticVersion -version $newVersion

    if ($oldParts.Major -ge 1) {
        if ($newParts.Major -ne $oldParts.Major) { return 'breaking' }
        if ($newParts.Minor -ne $oldParts.Minor) { return 'non-breaking' }
        return 'patch'
    }
    if ($oldParts.Minor -ge 1) {
        if ($newParts.Minor -ne $oldParts.Minor) { return 'breaking' }
        return 'patch'
    }
    return 'breaking'
}

function Test-IsBreakingChange {
    param(
        [string]$oldVersion,
        [ValidateSet('breaking', 'non-breaking', 'patch')]
        [string]$ChangeType
    )

    $parts = Split-SemanticVersion -version $oldVersion

    if ($parts.Major -ge 1) {
        return $ChangeType -eq 'breaking'
    }
    if ($parts.Minor -ge 1) {
        return $ChangeType -eq 'breaking'
    }
    return $true
}

# Reads the [package] table's `version = "..."` from a Cargo.toml on disk.
function Get-CurrentVersion {
    param([string]$cargoTomlPath)

    if (-not (Test-Path $cargoTomlPath)) {
        Write-Error "Could not find Cargo.toml file at '$cargoTomlPath'." -ErrorAction Stop
    }

    $cargoContent = Get-Content $cargoTomlPath -Raw
    $currentVersionMatch = $script:CargoPackageVersionRegex.Match($cargoContent)
    if (-not $currentVersionMatch.Success) {
        Write-Error "Could not determine [package] version from '$cargoTomlPath'." -ErrorAction Stop
    }

    return $currentVersionMatch.Groups[2].Value
}

# Reads the [package] `version = "..."` from a package's Cargo.toml at $BaseRef.
# Returns $null if the file does not exist at that ref (e.g. package added in this PR).
#
# Cached for the lifetime of the script run: $BaseRef is fixed by the caller
# for the entire run and the script never makes git commits, so the result
# for a given (BaseRef, PackageFolder) pair is invariant. Saves N×`git show`
# spawns per `Invoke-PlanReview` loop iteration (the dominant cost of the
# "Analyzing packages..." pause on Windows).
function Get-PackageVersionFromRef {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef,
        [Parameter(Mandatory = $true)][string]$PackageFolder
    )

    if ($null -eq $script:PackageVersionAtRefCache) {
        $script:PackageVersionAtRefCache = @{}
    }
    $cacheKey = "$RepoRoot`u{2402}$BaseRef`u{2402}$PackageFolder"
    if ($script:PackageVersionAtRefCache.ContainsKey($cacheKey)) {
        return $script:PackageVersionAtRefCache[$cacheKey]
    }

    $output = Invoke-Git -Arguments @('show', "${BaseRef}:crates/$PackageFolder/Cargo.toml") -RepoRoot $RepoRoot -AllowFailure
    $result = $null
    if ($null -ne $output) {
        $content = ($output -join "`n")
        $m = $script:CargoPackageVersionRegex.Match($content)
        if ($m.Success) { $result = $m.Groups[2].Value }
    }

    $script:PackageVersionAtRefCache[$cacheKey] = $result
    return $result
}

# --- WORKSPACE METADATA ---

# Cached `cargo metadata --no-deps` for the workspace. Graph topology is safe to cache
# across nested release runs; mutable version data is read fresh from disk via
# Get-CurrentVersion to avoid staleness.
$script:CachedWorkspaceMetadata = $null

# Caches for git-derived data that is invariant for the entire script run.
# These are valid for the whole release-packages.ps1 invocation because:
#   - $BaseRef is fixed by the caller for the entire run, and
#   - the script never makes git commits (HEAD does not move).
# Therefore the per-package baseline commit, the per-package committed-changes
# diff, and the per-package version-at-BaseRef are all stable for the whole
# session. They are populated lazily (first hit) and cleared only by
# Reset-ReleaseScriptCaches — NOT by the routine, mid-flow
# Invalidate-WorkspaceMetadataCache calls that the cascade fires after each
# in-memory Cargo.toml edit (those edits change cargo metadata's view of
# on-disk versions but leave git history untouched).
$script:PackageLastReleaseBaselineCache = $null
$script:PackageCommittedChangesCache    = $null
$script:PackageVersionAtRefCache        = $null

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

# Invalidates the cached metadata. Call this after editing any Cargo.toml in the
# workspace so subsequent analyses see fresh deps/versions.
#
# Intentionally does NOT clear the git-derived caches
# (PackageLastReleaseBaselineCache, PackageCommittedChangesCache,
# PackageVersionAtRefCache) — those are keyed on git history, which the
# release script never mutates (no commits are made). Test isolation
# between scenarios should call Reset-ReleaseScriptCaches instead, which
# clears every cache including this one.
function Invalidate-WorkspaceMetadataCache {
    $script:CachedWorkspaceMetadata = $null
}

# Clears every script-scoped cache used by the release tooling: workspace
# metadata AND the git-derived per-package caches (baseline commit, committed
# changes, version-at-BaseRef). Intended for test isolation between
# scenarios that build distinct synthetic workspaces — production code uses
# Invalidate-WorkspaceMetadataCache for the routine mid-flow invalidation
# after Cargo.toml edits.
function Reset-ReleaseScriptCaches {
    $script:CachedWorkspaceMetadata       = $null
    $script:PackageLastReleaseBaselineCache = $null
    $script:PackageCommittedChangesCache    = $null
    $script:PackageVersionAtRefCache        = $null
}

# Returns information about all workspace packages as an array of objects with:
#   Name                  - cargo package name
#   Folder                - folder name under crates/ (used as the script's PackageName argument)
#   Published             - $true if the package is published to crates.io
#   Deps                  - array of normalized dependency names (kind 'normal' or 'build', not 'dev')
#   AllowedExternalTypes  - array of strings from [package.metadata.cargo_check_external_types],
#                           or $null if the package does not declare them.
function Get-WorkspacePackages {
    param([string]$repoRoot)

    $metadata = Get-WorkspaceMetadata -repoRoot $repoRoot
    $cratesDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "crates"))

    $packages = @()
    foreach ($package in $metadata.packages) {
        $manifestDir = [System.IO.Path]::GetFullPath((Split-Path $package.manifest_path -Parent))
        if (-not $manifestDir.StartsWith($cratesDir, [System.StringComparison]::OrdinalIgnoreCase)) {
            continue
        }

        $deps = @()
        foreach ($dep in $package.dependencies) {
            if ($dep.kind -ne 'dev') {
                $deps += $dep.name.Replace('-', '_')
            }
        }

        $allowedTypes = $null
        $pkgMeta = $package.PSObject.Properties['metadata']
        if ($pkgMeta -and $null -ne $pkgMeta.Value) {
            $cet = $pkgMeta.Value.PSObject.Properties['cargo_check_external_types']
            if ($cet -and $null -ne $cet.Value) {
                $aet = $cet.Value.PSObject.Properties['allowed_external_types']
                if ($aet -and $null -ne $aet.Value) {
                    $allowedTypes = @($aet.Value)
                }
            }
        }

        $packages += [pscustomobject]@{
            Name                 = $package.name
            Folder               = Split-Path $manifestDir -Leaf
            Version              = $package.version
            Published            = -not ($null -ne $package.publish -and $package.publish.Count -eq 0)
            Deps                 = $deps
            AllowedExternalTypes = $allowedTypes
        }
    }

    return $packages
}

# Returns $true if the dependent package exposes any type rooted at the target package
# in its public API, as declared by [package.metadata.cargo_check_external_types].
# Conservative when metadata is missing.
function Test-PackageExposesTarget {
    param(
        [pscustomobject]$dependent,
        [string]$targetPackageName
    )

    if ($null -eq $dependent.AllowedExternalTypes) {
        return $true
    }

    $normalizedTarget = $targetPackageName.Replace('-', '_')
    foreach ($entry in $dependent.AllowedExternalTypes) {
        $root = ($entry -split '::', 2)[0]
        if ($root -eq $normalizedTarget) {
            return $true
        }
    }

    return $false
}

# BFS over the reverse dependency graph. Returns the folder names of all published
# workspace packages that depend on the given target (transitively) via [dependencies]
# or [build-dependencies]. The target itself is not included.
function Get-AllTransitiveDependents {
    param(
        [string]$packageName,
        [string]$repoRoot
    )

    $packages = Get-WorkspacePackages -repoRoot $repoRoot

    $targetPackage = $packages | Where-Object { $_.Folder -eq $packageName -or $_.Name -eq $packageName } | Select-Object -First 1
    if ($null -eq $targetPackage) {
        Write-Warning "Package '$packageName' not found in workspace metadata; cannot compute dependents."
        return @()
    }
    $normalizedTarget = $targetPackage.Name.Replace('-', '_')

    $toVisit = [System.Collections.Generic.Queue[string]]::new()
    $toVisit.Enqueue($normalizedTarget)
    $visited = [System.Collections.Generic.HashSet[string]]::new()
    [void]$visited.Add($normalizedTarget)

    $dependents = @()
    while ($toVisit.Count -gt 0) {
        $current = $toVisit.Dequeue()
        foreach ($candidate in $packages) {
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

    return $dependents
}

# --- FILE-CHANGE ANALYSIS ---

# Returns the package folder name (under crates/) that contains the given repo-relative
# path, or $null if the path is outside any package.
function Get-PackageFolderForPath {
    param([string]$Path)

    $normalized = $Path.Replace('\', '/')
    if (-not $normalized.StartsWith('crates/')) { return $null }
    $rest = $normalized.Substring('crates/'.Length)
    $slash = $rest.IndexOf('/')
    if ($slash -le 0) { return $null }
    return $rest.Substring(0, $slash)
}

# Returns the SHA of the most recent commit that touched the `version =` or
# `publish =` line in the package's Cargo.toml, or $null if no such commit exists
# in the package's committed history. This is the per-package "last release boundary":
# any change under crates/<folder>/ newer than this commit is unreleased from the
# perspective of crates.io, regardless of which PR introduced it.
#
# We intentionally do not rely on git tags. The repo creates them after merge to
# main, but a CI-time clone or a partial fetch may not have them, and a tag is
# a side effect of a release while the Cargo.toml edit is the cause.
#
# Cached for the lifetime of the script run (the script never commits, so the
# baseline SHA per folder is invariant). The cache is cleared by
# Reset-ReleaseScriptCaches between test scenarios; production mid-flow
# invalidations (Invalidate-WorkspaceMetadataCache) deliberately leave it alone.
function Get-PackageLastReleaseBaseline {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$PackageFolder
    )

    if ($null -eq $script:PackageLastReleaseBaselineCache) {
        $script:PackageLastReleaseBaselineCache = @{}
    }
    $cacheKey = "$RepoRoot`u{2402}$PackageFolder"
    if ($script:PackageLastReleaseBaselineCache.ContainsKey($cacheKey)) {
        return $script:PackageLastReleaseBaselineCache[$cacheKey]
    }

    $relPath = "crates/$PackageFolder/Cargo.toml"
    # -G matches any added/removed diff line whose content matches the regex.
    # Anchoring at column 0 keeps us on top-level keys, not version-like strings
    # appearing inside dependency tables or arbitrary literals. We accept the
    # dotted-key TOML variants `publish.workspace = true` and
    # `version.workspace = true` (which inherit from the workspace root) in
    # addition to the literal inline forms `publish = ...` and `version = ...`
    # (which already match the `(version|publish)` group whether the
    # right-hand side is a literal, an array, or an inline table like
    # `{ workspace = true }`). NOTE: this pattern is a POSIX ERE — git's `-G`
    # flag does not accept PCRE extensions like `(?:...)`, so we use a
    # capturing group for the optional `.workspace` suffix instead.
    $out = Invoke-Git -Arguments @('log', '-1', '--format=%H', '-G', '^(version|publish)(\.workspace)?\s*=', '--', $relPath) -RepoRoot $RepoRoot -AllowFailure
    $result = $null
    if ($null -ne $out) {
        $sha = (@($out))[0]
        if (-not [string]::IsNullOrWhiteSpace($sha)) {
            $result = $sha.ToString().Trim()
        }
    }

    $script:PackageLastReleaseBaselineCache[$cacheKey] = $result
    return $result
}

# Returns the list of repo-relative paths under crates/<PackageFolder>/ that
# have changed in committed history between the package's last release baseline
# (see Get-PackageLastReleaseBaseline) and HEAD. Returns an empty array if the
# package has no prior release boundary recorded.
#
# Cached for the lifetime of the script run (the script never commits, so the
# committed diff per folder is invariant). The cache is cleared by
# Reset-ReleaseScriptCaches between test scenarios.
function Get-PackageCommittedChanges {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$PackageFolder
    )

    if ($null -eq $script:PackageCommittedChangesCache) {
        $script:PackageCommittedChangesCache = @{}
    }
    $cacheKey = "$RepoRoot`u{2402}$PackageFolder"
    if ($script:PackageCommittedChangesCache.ContainsKey($cacheKey)) {
        return $script:PackageCommittedChangesCache[$cacheKey]
    }

    $baseline = Get-PackageLastReleaseBaseline -RepoRoot $RepoRoot -PackageFolder $PackageFolder
    $paths = New-Object 'System.Collections.Generic.List[string]'
    if (-not [string]::IsNullOrEmpty($baseline)) {
        $committed = Invoke-Git -Arguments @('diff', '--name-only', $baseline, 'HEAD', '--', "crates/$PackageFolder") -RepoRoot $RepoRoot
        foreach ($line in $committed) {
            $p = $line.ToString().Trim().Replace('\', '/')
            if (-not [string]::IsNullOrEmpty($p)) { $paths.Add($p) }
        }
    }
    $result = $paths.ToArray()

    $script:PackageCommittedChangesCache[$cacheKey] = $result
    return $result
}

# For each published workspace package, returns a hashtable folder -> ChangedFileCount
# where the count is the number of distinct repo-relative paths under crates/<folder>/
# that have changed since the package's last release baseline (see
# Get-PackageLastReleaseBaseline). Considers:
#
#   - committed changes between the baseline and HEAD,
#   - tracked working-tree edits (staged + unstaged) vs HEAD,
#   - untracked files (e.g. new source files added during a release run).
#
# Packages with zero modifications are omitted from the result.
#
# Working-tree edits and untracked files are queried once globally and bucketed
# per package to avoid spawning O(packages) extra git processes. The per-package
# committed diff is served from Get-PackageCommittedChanges' session cache.
function Get-PackagesWithUnreleasedChanges {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    $result = @{}
    $packages = Get-WorkspacePackages -repoRoot $RepoRoot

    $workingByPackage = @{}
    $globalWorking   = Invoke-Git -Arguments @('diff', '--name-only', 'HEAD', '--') -RepoRoot $RepoRoot
    $globalUntracked = Invoke-Git -Arguments @('ls-files', '--others', '--exclude-standard') -RepoRoot $RepoRoot
    foreach ($line in @(@($globalWorking) + @($globalUntracked))) {
        $p = $line.ToString().Trim().Replace('\', '/')
        if ([string]::IsNullOrEmpty($p)) { continue }
        $folder = Get-PackageFolderForPath -Path $p
        if (-not $folder) { continue }
        if (-not $workingByPackage.ContainsKey($folder)) {
            $workingByPackage[$folder] = [System.Collections.Generic.HashSet[string]]::new()
        }
        [void]$workingByPackage[$folder].Add($p)
    }

    foreach ($package in $packages) {
        if (-not $package.Published) { continue }

        $folder = $package.Folder
        $files = [System.Collections.Generic.HashSet[string]]::new()

        foreach ($p in Get-PackageCommittedChanges -RepoRoot $RepoRoot -PackageFolder $folder) {
            [void]$files.Add($p)
        }

        if ($workingByPackage.ContainsKey($folder)) {
            foreach ($p in $workingByPackage[$folder]) { [void]$files.Add($p) }
        }

        if ($files.Count -gt 0) {
            $result[$folder] = $files.Count
        }
    }

    return $result
}

# For every published workspace package, compares the on-disk current version with the
# version at $BaseRef and returns the folders whose version differs. On-disk reads
# avoid cache staleness when this is called between mid-run Cargo.toml edits.
function Get-PackagesWithVersionChanges {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef
    )

    $packages = Get-WorkspacePackages -repoRoot $RepoRoot
    $changed = [System.Collections.Generic.HashSet[string]]::new()

    foreach ($package in $packages) {
        if (-not $package.Published) { continue }

        $cargoToml = Join-Path $RepoRoot "crates/$($package.Folder)/Cargo.toml"
        if (-not (Test-Path $cargoToml)) { continue }

        $currentVersion = Get-CurrentVersion -cargoTomlPath $cargoToml
        $baseVersion    = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $package.Folder

        # New package (not present at base) counts as version-changed (it is
        # being released for the first time).
        if ($null -eq $baseVersion) {
            [void]$changed.Add($package.Folder)
            continue
        }

        if ($currentVersion -ne $baseVersion) {
            [void]$changed.Add($package.Folder)
        }
    }

    # PowerShell pipeline collapses an empty HashSet to $null on return; -NoEnumerate
    # preserves it so callers' .Contains() calls still work.
    Write-Output -NoEnumerate $changed
}

# Returns a sorted array of pending-release records for every published workspace
# package whose on-disk Cargo.toml version differs from the version at $BaseRef. Each
# record exposes the data the announcement formatter and base-relative re-invocation
# logic need:
#
#   [pscustomobject]@{
#     Folder         = '<package folder under crates/>'
#     Name           = '<package name from Cargo.toml [package].name>'
#     BaseVersion    = '<version at BaseRef>'
#     CurrentVersion = '<version on disk>'
#   }
#
# New packages not present at $BaseRef are NOT included — they have no "base version"
# to compare against, and the rest of the script's flow treats them as fresh
# releases anyway (Invoke-PackageRelease writes the initial Cargo.toml + changelog
# entry). Only packages that genuinely have a prior committed version with a
# different on-disk version qualify as "pending" in the cross-invocation sense.
#
# Sorted ascending by Folder for deterministic output (the announcement order
# must be stable across runs / hosts / etc.).
function Get-PendingReleases {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$BaseRef
    )

    $packages = Get-WorkspacePackages -repoRoot $RepoRoot
    $pending = New-Object System.Collections.Generic.List[object]

    foreach ($package in $packages) {
        if (-not $package.Published) { continue }

        $cargoToml = Join-Path $RepoRoot "crates/$($package.Folder)/Cargo.toml"
        if (-not (Test-Path $cargoToml)) { continue }

        $currentVersion = Get-CurrentVersion -cargoTomlPath $cargoToml
        $baseVersion    = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $package.Folder

        # New package at base: skip (no base version to be pending against).
        if ($null -eq $baseVersion) { continue }
        if ($currentVersion -eq $baseVersion) { continue }

        $pending.Add([pscustomobject]@{
            Folder         = $package.Folder
            Name           = $package.Name
            BaseVersion    = $baseVersion
            CurrentVersion = $currentVersion
        }) | Out-Null
    }

    return @($pending | Sort-Object -Property Folder)
}

# Builds a ResolvedReleaseSet (folder -> resolved entry) from base-ref vs disk
# version diffing. Used by callers that have no explicit user-input release
# plan to feed Get-UnreleasedModifiedDependencies — currently
# scripts/check-unreleased-dependencies.ps1 (CI scan, no user input).
#
# Every member is marked Source='cascade' so the elevation-surface predicate
# in Get-UnreleasedModifiedDependencies treats every release-set member as
# potentially-elevatable. This matches the bundled-input semantics: in
# the absence of explicit user intent, every below-breaking release-set
# member is surfaced for review.
#
# New packages (no version at $BaseRef) are tagged 'breaking' so the
# elevation predicate skips them — they have no prior version transition to
# elevate. This matches the pre-refactor null-base-version guard behavior.
function New-ResolvedReleaseSetFromBaseRef {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string]$BaseRef
    )

    $resolved = @{}
    $folders = Get-PackagesWithVersionChanges -RepoRoot $RepoRoot -BaseRef $BaseRef
    if ($null -eq $folders -or $folders.Count -eq 0) { return $resolved }

    $packages = Get-WorkspacePackages -repoRoot $RepoRoot
    $pkgByFolder = @{}
    foreach ($p in $packages) { $pkgByFolder[$p.Folder] = $p }

    foreach ($folder in $folders) {
        if (-not $pkgByFolder.ContainsKey($folder)) { continue }
        $pkg = $pkgByFolder[$folder]
        $baseVersion = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $folder
        $changeType = if ($null -eq $baseVersion) {
            # New package: no semantically-meaningful prior version to elevate from.
            'breaking'
        } else {
            Get-ChangeTypeFromVersions -oldVersion $baseVersion -newVersion $pkg.Version
        }
        $resolved[$folder] = [pscustomobject]@{
            Folder                  = $folder
            Name                    = $pkg.Name
            CurrentVersion          = $baseVersion
            EffectiveChangeType     = $changeType
            EffectiveTargetVersion  = $pkg.Version
            Source                  = 'cascade'
            AutoUpgraded            = $false
            CascadeReasons          = New-Object 'System.Collections.Generic.List[object]'
        }
    }

    return $resolved
}

# --- CORE ANALYSIS ---
#
# Upholds the CASCADE-ORGANIZATION INVARIANTS documented in docs/releasing.md
# under "Cascade Organisation Invariants":
#   (A) A cascade toward dependents never introduces items to the user-review
#       queue. Honored via the optional -ModifiedSnapshot parameter: when
#       callers capture the modifications set BEFORE the primary release
#       runs and pass it in, cascade-only targets (those whose only
#       modification is the cascade-written Cargo.toml / CHANGELOG.md) never
#       enter the snapshot and so cannot surface as findings on later
#       iterations.
#   (B) A release-set member whose cascade-applied change type is below the
#       semantic maximum (breaking) and which has pre-existing modifications
#       is reported so the user can still elevate the change type after
#       reviewing the changes. User-source members (Source='user' in the
#       resolved set) carry an explicit decision and are NOT re-prompted —
#       elevation review applies only to cascade-source members.
#
# For each package in the "resolved release set" (passed in by the caller as a
# folder -> resolved-entry hashtable produced by Resolve-ReleaseSet, or by
# tests via the New-ResolvedReleaseSetFromBaseRef helper), walk its transitive
# normal/build workspace dependencies. Report any workspace dependency that
#
#   1. has source modifications since its own last release baseline (i.e. since the
#      most recent commit that touched its `version =` or `publish =` line — see
#      Get-PackageLastReleaseBaseline), and
#   2. is either (a) NOT itself in the release set, OR (b) IS in the release set
#      as a cascade-source member whose EffectiveChangeType is below "breaking"
#      (so the user might still want to elevate it after reviewing the changes), and
#   3. is published (publish != false),
#
# along with the shortest dependency chain that reaches it from a released package.
#
# Per-package baselines (rather than a global PR-vs-base-ref diff) are required to
# detect transitive dependency changes that were merged to main in earlier PRs without
# a version change and are now being depended on by a release-set package in this PR.
# Comparing the working tree only against the PR base ref would miss those.
#
# Returns @() when there are no findings, otherwise an array of objects:
#   Folder            - package folder under crates/
#   PackageName       - cargo package name
#   CurrentVersion    - package's current version (Cargo.toml [package].version)
#   InReleaseSet      - $true when the finding is also a release-set member
#                       surfaced for cascade elevation review (Source='cascade'
#                       with below-breaking change type); $false otherwise.
#                       The caller uses this to distinguish "needs review for
#                       elevation" from "needs review for primary release".
#   ChangedFileCount  - number of files changed under crates/<folder>/ since baseline
#   DependencyChains  - @( @('released_package', 'mid_package', 'this_dep'), ... )
#                       - chains rooted in release-set members (or, in
#                       -IncludeAllModifiedAsRoots mode, also in other
#                       modified-published packages) that transitively reach
#                       `this_dep`. Used by the PR sticky comment to highlight
#                       what is at risk in the current release plan specifically.
#   WorkspaceDependencyChains  - @( @('top_dependent', ..., 'this_dep'), ... )
#                       - every path in the workspace dep graph ending at
#                       `this_dep`, irrespective of release-set membership.
#                       Used by the interactive per-package menu to give the
#                       reviewer a release-set-independent "big picture" view
#                       of what could be affected by releasing this package.
#
# The BFS traverses past every node (including release-set members) so a chain
# like 'foo -> bar -> baz' is recorded even when 'bar' is itself being
# released. Chains are then reduced (deduped + suffix-subsumed) so a shorter
# chain that is a strict suffix of a longer one (e.g. 'bar -> baz' vs
# 'foo -> bar -> baz') is dropped to keep the prompt focused on the longest
# path from each release-set entry point.
function Get-UnreleasedModifiedDependencies {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][hashtable]$ResolvedReleaseSet,
        [Parameter(Mandatory = $false)][hashtable]$ModifiedSnapshot,
        # When set, treats every modified-published package as an additional BFS
        # root (in addition to ResolvedReleaseSet members) so chains BETWEEN
        # changed packages surface naturally, AND sweeps any modified-published
        # package the surfacing predicate accepts but no BFS run reached as a
        # dep, adding it as a "stub" finding (DependencyChains = @()). Used by
        # the guided changed-packages workflow (release-packages.ps1 -Changed / -All).
        [switch]$IncludeAllModifiedAsRoots
    )

    $packages = Get-WorkspacePackages -repoRoot $RepoRoot
    # Use the caller-provided snapshot when present so Invariant A holds across
    # cascade writes (which would otherwise pollute Get-PackagesWithUnreleasedChanges's
    # working-tree query and surface cascade-only targets as findings).
    $modifiedMap = if ($PSBoundParameters.ContainsKey('ModifiedSnapshot') -and $null -ne $ModifiedSnapshot) {
        $ModifiedSnapshot
    } else {
        Get-PackagesWithUnreleasedChanges -RepoRoot $RepoRoot
    }

    if ($IncludeAllModifiedAsRoots) {
        if ($ResolvedReleaseSet.Count -eq 0 -and $modifiedMap.Count -eq 0) { return @() }
    } else {
        if ($ResolvedReleaseSet.Count -eq 0) { return @() }
    }

    # Build folder -> package lookup and normalized-name -> folder lookup.
    $byFolder = @{}
    $folderByNormName = @{}
    foreach ($c in $packages) {
        $byFolder[$c.Folder] = $c
        $folderByNormName[$c.Name.Replace('-', '_')] = $c.Folder
    }

    # Local closure: decide whether a modified-published package should surface
    # as a finding given its release-set membership. Centralised so the BFS
    # body (which checks a *visited dep*) and the Phase B sweep (which checks
    # a *root*) share the same predicate. Surface when (modified + published)
    # AND either:
    #   - not a release-set member (classic case), OR
    #   - a release-set member with Source='cascade' whose EffectiveChangeType
    #     is below "breaking" (Invariant B — elevation review). Source='user'
    #     members carry an explicit decision from the CLI input and are NOT
    #     re-prompted.
    $shouldSurface = {
        param([string]$folder)
        $pkg = $byFolder[$folder]
        if ($null -eq $pkg) { return $false }
        if (-not ($modifiedMap.ContainsKey($folder) -and $pkg.Published)) { return $false }
        $entry = $ResolvedReleaseSet[$folder]
        if ($null -eq $entry) { return $true }
        return ($entry.Source -eq 'cascade' -and $entry.EffectiveChangeType -ne 'breaking')
    }.GetNewClosure()

    # Aggregate findings: folder -> { Folder; PackageName; ChangedFileCount; DependencyChains }.
    # Ordered so the BFS insertion order is preserved when iterating .Values; matters because
    # the post-release scan prompts the user in this order and a non-deterministic order
    # makes the UX flaky and tests unreliable.
    $findings = [ordered]@{}

    # Compute BFS roots. In the default (targeted) mode they're the
    # release-set members. When -IncludeAllModifiedAsRoots is set we also add
    # every modified-published package so chains between changed packages can
    # be recorded (e.g. 'bytesbuf_io -> bytesbuf' when both are changed and
    # bytesbuf_io depends on bytesbuf). Sorted for deterministic prompt order.
    $rootFolders = if ($IncludeAllModifiedAsRoots) {
        $set = [System.Collections.Generic.HashSet[string]]::new()
        foreach ($k in $ResolvedReleaseSet.Keys) { [void]$set.Add($k) }
        foreach ($k in $modifiedMap.Keys) {
            $pkg = $byFolder[$k]
            if ($null -ne $pkg -and $pkg.Published) { [void]$set.Add($k) }
        }
        @($set | Sort-Object)
    } else {
        @($ResolvedReleaseSet.Keys | Sort-Object)
    }

    foreach ($releasedFolder in $rootFolders) {
        if (-not $byFolder.ContainsKey($releasedFolder)) { continue }

        # BFS forward over normal+build deps. Track shortest path to each visited
        # node within this start-package's traversal (avoids cycles and keeps the
        # recorded chain to the SHORTEST path from this entry point).
        $visited = [System.Collections.Generic.HashSet[string]]::new()
        [void]$visited.Add($releasedFolder)
        $queue = [System.Collections.Generic.Queue[object]]::new()
        $queue.Enqueue([pscustomobject]@{ Folder = $releasedFolder; Chain = @($releasedFolder) })

        while ($queue.Count -gt 0) {
            $node = $queue.Dequeue()
            $package = $byFolder[$node.Folder]
            if ($null -eq $package) { continue }

            foreach ($depNorm in $package.Deps) {
                if (-not $folderByNormName.ContainsKey($depNorm)) { continue } # external package
                $depFolder = $folderByNormName[$depNorm]
                if ($visited.Contains($depFolder)) { continue }
                [void]$visited.Add($depFolder)

                $depPackage = $byFolder[$depFolder]
                $depChain = $node.Chain + $depFolder

                if (& $shouldSurface $depFolder) {
                    $depEntry = $ResolvedReleaseSet[$depFolder]
                    $isInReleaseSet = $null -ne $depEntry
                    if (-not $findings.Contains($depFolder)) {
                        $findings[$depFolder] = [pscustomobject]@{
                            Folder           = $depFolder
                            PackageName      = $depPackage.Name
                            CurrentVersion   = $depPackage.Version
                            InReleaseSet     = $isInReleaseSet
                            ChangedFileCount = $modifiedMap[$depFolder]
                            DependencyChains = @(, $depChain)
                        }
                    }
                    else {
                        $existing = $findings[$depFolder]
                        $existing.DependencyChains = @($existing.DependencyChains) + @(, $depChain)
                    }
                }

                # Traverse past every node — release-set members, unchanged
                # intermediates, and recorded findings alike. This lets us
                # surface chains that thread through release-set members to a
                # deeper modified-and-unreleased target (e.g. 'foo -> bar -> baz'
                # where 'bar' is being released and 'baz' is not).
                $queue.Enqueue([pscustomobject]@{ Folder = $depFolder; Chain = $depChain })
            }
        }
    }

    # Phase B sweep (all-changed mode only): every modified-published package
    # the surfacing predicate accepts but no BFS run reached as a dep gets
    # added as a stub finding (empty chains). This is what implements the
    # "imaginary `*` package depends on every changed package" UX without
    # introducing a sentinel: a stub-with-empty-chains finding renders as
    # "No dependents in release set" in the menu.
    if ($IncludeAllModifiedAsRoots) {
        foreach ($folder in @($modifiedMap.Keys | Sort-Object)) {
            if ($findings.Contains($folder)) { continue }
            if (-not (& $shouldSurface $folder)) { continue }
            $pkg = $byFolder[$folder]
            $findings[$folder] = [pscustomobject]@{
                Folder           = $folder
                PackageName      = $pkg.Name
                CurrentVersion   = $pkg.Version
                InReleaseSet     = $ResolvedReleaseSet.ContainsKey($folder)
                ChangedFileCount = $modifiedMap[$folder]
                DependencyChains = @()
            }
        }
    }

    if ($findings.Count -eq 0) { return @() }

    # Reduce each finding's chains: drop duplicates and shorter chains that are
    # strict suffixes of a longer chain, so the user sees only the longest
    # caller-rooted path through each branch.
    foreach ($f in $findings.Values) {
        if ($null -ne $f.DependencyChains -and @($f.DependencyChains).Count -gt 0) {
            $f.DependencyChains = Reduce-DependencyChains -Chains $f.DependencyChains
        }
    }

    # Populate WorkspaceDependencyChains: every path in the workspace dep graph
    # of the form `[root, ..., target]` ending at this finding's folder. Used
    # by the interactive menu to give the user a release-set-independent
    # picture of what could be affected by releasing the package under review
    # (cascading can pull more dependents into the release set after the
    # review prompt, so the release-set-rooted DependencyChains list would
    # otherwise be misleadingly narrow). Computed here (not at menu render
    # time) so $packages is reused and no extra cargo metadata invocations
    # happen per prompt.
    foreach ($f in $findings.Values) {
        $f | Add-Member -NotePropertyName WorkspaceDependencyChains -NotePropertyValue (
            Get-InWorkspaceDependencyChains -Packages $packages -TargetFolder $f.Folder
        ) -Force
    }

    return @($findings.Values)
}

# Deduplicates dependency chains and drops chains that are strict suffixes of
# any other kept chain. Returns a stable-sorted array (alphabetical by joined
# chain text) so the UX prompt and the PR comment render deterministically.
#
# A chain X is "subsumed by" chain Y when Y is strictly longer than X and X
# equals the tail of Y element-for-element. Subsumption is one-directional —
# we keep the LONGER chain because it carries strictly more context for the
# reviewer (the same suffix plus its caller ancestry).
function Reduce-DependencyChains {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [object[]]$Chains
    )

    if ($null -eq $Chains -or $Chains.Count -eq 0) { return @() }

    # Step 1: dedupe by canonical string key (preserves the first occurrence).
    $seen = [ordered]@{}
    foreach ($c in $Chains) {
        $arr = @($c)
        $key = $arr -join "`u{2192}" # rightwards arrow as a separator unlikely to collide
        if (-not $seen.Contains($key)) { $seen[$key] = $arr }
    }
    $unique = @($seen.Values)

    # Step 2: sort by length descending and keep each chain only when no
    # already-kept (longer) chain has it as a strict suffix.
    $sortedByLengthDesc = @($unique | Sort-Object @{ Expression = { $_.Length }; Descending = $true })
    $kept = New-Object System.Collections.Generic.List[object]
    foreach ($c in $sortedByLengthDesc) {
        $isSuffix = $false
        foreach ($k in $kept) {
            if ($c.Length -ge $k.Length) { continue } # strict suffix requires shorter length
            $offset = $k.Length - $c.Length
            $match = $true
            for ($i = 0; $i -lt $c.Length; $i++) {
                if ($c[$i] -ne $k[$offset + $i]) { $match = $false; break }
            }
            if ($match) { $isSuffix = $true; break }
        }
        if (-not $isSuffix) { [void]$kept.Add($c) }
    }

    # Step 3: stable alphabetical sort by joined chain text so output order
    # is deterministic across runs and across release-set iteration order.
    $finalSorted = @($kept | Sort-Object { ($_ -join ' -> ') })
    # IMPORTANT: prefix the return with `,` to prevent PowerShell from
    # unwrapping a single-element array-of-arrays into its inner array,
    # which would silently corrupt $finding.DependencyChains[0] when only
    # one chain survives reduction (caller would see a flat string array
    # instead of an array containing one chain).
    return ,$finalSorted
}

# Computes the set of in-workspace dependency chains that end at $TargetFolder
# - i.e. every path through the workspace package dep graph of the form
# `[root, ..., target]` where `root` is some workspace package that
# transitively depends on `target` and `root` itself has no in-workspace
# dependent (the chain reaches as far up the dependency tree as possible).
# Used by `Format-PackageMenu` to give the user a "big picture" view of what
# could be affected by releasing the package under review - independent of
# which packages are in the current release set, since cascading can bring
# in more dependents after the review prompt is shown.
#
# `$Packages` is the already-loaded workspace package list (output of
# `Get-WorkspacePackages`); pass it in to avoid re-running `cargo metadata`
# when the caller already has it.
#
# Returns @() when $TargetFolder is unknown, or when no other workspace
# package transitively depends on it. Otherwise returns chains reduced via
# `Reduce-DependencyChains` (suffix-subsumed shorter chains dropped). Dev
# dependencies and non-`crates/` workspace members are NOT included, since
# `Get-WorkspacePackages` already filters them out - this matches the
# release-impact semantics we care about (dev-dep changes don't affect a
# package's published-API consumers).
function Get-InWorkspaceDependencyChains {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [object[]]$Packages,
        [Parameter(Mandatory = $true)][string]$TargetFolder
    )

    # PowerShell unwraps a bare `return @()` to $null at the function
    # boundary (the empty array contributes 0 items to the output stream).
    # Prefix returns with `,` to force an array-preserving single-item
    # output - the receiver sees the array (possibly empty), not $null.
    if ($null -eq $Packages -or $Packages.Count -eq 0) { return ,@() }

    # Build folder -> package and normalized-name -> folder lookups (same shape
    # the BFS in Get-UnreleasedModifiedDependencies builds for forward edges).
    $byFolder = @{}
    $folderByNormName = @{}
    foreach ($p in $Packages) {
        $byFolder[$p.Folder] = $p
        $folderByNormName[$p.Name.Replace('-', '_')] = $p.Folder
    }
    if (-not $byFolder.ContainsKey($TargetFolder)) { return ,@() }

    # Reverse adjacency: depFolder -> list of folders that depend on depFolder.
    $reverse = @{}
    foreach ($p in $Packages) {
        foreach ($depNorm in $p.Deps) {
            if (-not $folderByNormName.ContainsKey($depNorm)) { continue } # external
            $depFolder = $folderByNormName[$depNorm]
            if (-not $reverse.ContainsKey($depFolder)) {
                $reverse[$depFolder] = New-Object 'System.Collections.Generic.List[string]'
            }
            [void]$reverse[$depFolder].Add($p.Folder)
        }
    }

    # Iterative DFS over reverse edges starting at $TargetFolder. Each stack
    # entry carries the path-so-far in REVERSE order (target first, current
    # frontier last) so cycle detection is a quick membership check. When a
    # frontier has no further dependents (workspace root reached), we emit the
    # reversed path as a chain `[root, ..., target]`. Cycles can't exist in a
    # valid Cargo workspace, but defensive `notcontains` keeps the loop safe
    # if metadata ever yields one.
    $chains = New-Object 'System.Collections.Generic.List[object]'
    $stack = [System.Collections.Generic.Stack[object]]::new()
    $stack.Push([pscustomobject]@{
        Folder       = $TargetFolder
        ReversedPath = @($TargetFolder)
    })

    while ($stack.Count -gt 0) {
        $node = $stack.Pop()
        $candidates = @()
        if ($reverse.ContainsKey($node.Folder)) {
            foreach ($d in $reverse[$node.Folder]) {
                if ($node.ReversedPath -notcontains $d) { $candidates += $d }
            }
        }

        if ($candidates.Count -eq 0) {
            # Reached a top-level dependent (or all further dependents would
            # cycle). Skip the trivial single-element [target] "chain" - there
            # is nothing to display when target has no in-workspace dependents.
            if ($node.ReversedPath.Length -gt 1) {
                $chain = New-Object 'System.Collections.Generic.List[string]'
                for ($i = $node.ReversedPath.Length - 1; $i -ge 0; $i--) {
                    [void]$chain.Add($node.ReversedPath[$i])
                }
                [void]$chains.Add(@($chain))
            }
        } else {
            foreach ($d in $candidates) {
                $stack.Push([pscustomobject]@{
                    Folder       = $d
                    ReversedPath = $node.ReversedPath + $d
                })
            }
        }
    }

    if ($chains.Count -eq 0) { return ,@() }
    # Reduce-DependencyChains already returns ,$finalSorted, so its non-empty
    # array structure survives this forward.
    return Reduce-DependencyChains -Chains $chains
}
