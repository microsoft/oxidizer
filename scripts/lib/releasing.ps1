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

# Ordinal rank of a change type, used to compute the stronger of two change
# types. 'none' means "no constraint" (e.g. cargo-semver-checks found nothing to
# compare against) and ranks below every real change type.
$script:ChangeTypeRank = @{ 'none' = 0; 'patch' = 1; 'non-breaking' = 2; 'breaking' = 3 }

# Returns whichever of two change types is the stronger (higher-ranked). Unknown
# or empty inputs are treated as 'none' (rank 0). Ties return $A.
function Get-StrongerChangeType {
    param(
        [AllowNull()][AllowEmptyString()][string]$A,
        [AllowNull()][AllowEmptyString()][string]$B
    )
    $ra = $script:ChangeTypeRank[$A]; if ($null -eq $ra) { $ra = 0 }
    $rb = $script:ChangeTypeRank[$B]; if ($null -eq $rb) { $rb = 0 }
    if ($rb -gt $ra) { return $B }
    return $A
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

# Finds the crate's PREVIOUS version-bump commit: the most recent commit reachable
# from $BaseRef whose diff changed the `[package] version = "..."` line in
# crates/<PackageFolder>/Cargo.toml. Returns a [pscustomobject] with:
#   Sha     - the commit SHA (suitable for `cargo semver-checks --baseline-rev`)
#   Version - the [package] version declared at that commit
# or $null when no such commit exists (a brand-new crate introduced in the range
# above $BaseRef, or a crate with no committed history at $BaseRef).
#
# Genuine lookup FAILURES are NOT swallowed: if $BaseRef cannot be resolved (e.g.
# it was never fetched, or is a typo) or git otherwise fails, this THROWS rather
# than returning $null. A silent $null would become 'none' (no change-type floor)
# downstream and make the CI report incorrectly pass when the baseline could not
# actually be determined. The CI report catches the throw and records an
# ⚠️ unknown/warn row; the release planner treats it as a hard error. Only a
# SUCCESSFUL git log that finds no version-bump commit (a valid ref, but the
# crate's [package] version never changed in reachable history) yields $null.
#
# This is the SOURCE-LEVEL semver baseline: the version the repository previously
# *declared*, regardless of whether it was ever published to a registry. It works
# identically in OSS and enterprise/offline environments because it never touches
# crates.io — the baseline rustdoc is rebuilt from the crate's source at that
# commit by cargo-semver-checks' `--baseline-rev`.
#
# $BaseRef controls which bump is "previous":
#   - CI report: pass the PR base (e.g. origin/main) so THIS PR's own bump — which
#     lives only on the PR head — is excluded, and the baseline is the last bump
#     that already landed on the base branch.
#   - Release planner: pass HEAD so the baseline is the last committed version bump
#     (the previous release), compared against the working-tree API being released.
#
# Implementation: walk `git log <BaseRef> -- <Cargo.toml>` newest-first and, for
# each touching commit, compare the [package] version at the commit against the
# version at its parent (via Get-PackageVersionFromRef, which matches only the
# [package]-scoped version — not dependency-table versions). The first commit
# where they differ is the bump. Matching on the parsed [package] version (rather
# than a raw `-G` line-diff) means a commit that only edited a dependency's
# `version = "..."` or toggled `publish` is correctly skipped.
#
# Cached for the lifetime of the script run (the script never commits, so the
# result per (RepoRoot, BaseRef, PackageFolder) is invariant). Cleared by
# Reset-ReleaseScriptCaches between test scenarios.
function Get-PreviousVersionBumpCommit {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef,
        [Parameter(Mandatory = $true)][string]$PackageFolder
    )

    if ($null -eq $script:PreviousVersionBumpCommitCache) {
        $script:PreviousVersionBumpCommitCache = @{}
    }
    $cacheKey = "$RepoRoot`u{2402}$BaseRef`u{2402}$PackageFolder"
    if ($script:PreviousVersionBumpCommitCache.ContainsKey($cacheKey)) {
        return $script:PreviousVersionBumpCommitCache[$cacheKey]
    }

    $relPath = "crates/$PackageFolder/Cargo.toml"

    # Surface a genuine failure rather than silently returning "no baseline".
    # An unresolvable ref (not fetched / typo) must not be mistaken for a
    # brand-new crate — that would drop the change-type floor and let an
    # under-incremented release pass. Test-GitRef never throws; a false result
    # means the ref is bad. The git log itself runs WITHOUT -AllowFailure so any
    # other git error also propagates. A valid ref with no matching commit exits
    # 0 with empty output and correctly yields $null (new crate).
    if (-not (Test-GitRef -Ref $BaseRef -RepoRoot $RepoRoot)) {
        throw "Cannot locate the previous version-bump commit for '$PackageFolder': base ref '$BaseRef' could not be resolved in the repository. Ensure it is fetched (CI checks out with fetch-depth: 0) and spelled correctly."
    }
    $commits = Invoke-Git -Arguments @('log', '--format=%H', $BaseRef, '--', $relPath) -RepoRoot $RepoRoot

    $result = $null
    if ($null -ne $commits) {
        foreach ($line in @($commits)) {
            $sha = $line.ToString().Trim()
            if ([string]::IsNullOrWhiteSpace($sha)) { continue }

            $verAt = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef $sha -PackageFolder $PackageFolder
            if ($null -eq $verAt) { continue }

            # Version at the parent commit; $null when the crate did not exist there
            # (this commit introduced it) or when $sha is the repository root.
            $verParent = Get-PackageVersionFromRef -RepoRoot $RepoRoot -BaseRef "$sha^" -PackageFolder $PackageFolder

            if ($verAt -ne $verParent) {
                $result = [pscustomobject]@{ Sha = $sha; Version = $verAt }
                break
            }
        }
    }

    $script:PreviousVersionBumpCommitCache[$cacheKey] = $result
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
$script:PreviousVersionBumpCommitCache  = $null

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
    $script:PreviousVersionBumpCommitCache  = $null
    $script:CrateSemverVerdictCache         = $null
}

# Memoised, mockable classifier: returns the minimum change type a crate's
# current working-tree public API requires versus its previous version-bump
# commit ('breaking' / 'non-breaking' / 'patch' / 'none' when there is no prior
# bump to compare against). Ordinary library crates are classified by running
# cargo-semver-checks once per crate. Proc-macro-only crates have no supported
# cargo-semver-checks API surface, so they return the explicit 'manual' result
# before the tool is invoked; the interactive planner owns that decision.
# Resolve-ReleaseSet is invoked many times during the interactive review loop, so
# results are cached per cargo name for the run. Test suites Mock this function to
# supply deterministic verdicts without invoking the real tool (see the scenario
# harness).
function Get-CrateRequiredChangeType {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Folder,
        [Parameter(Mandatory = $true)][string]$CargoName,
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    if ($null -eq $script:CrateSemverVerdictCache) { $script:CrateSemverVerdictCache = @{} }
    if ($script:CrateSemverVerdictCache.ContainsKey($CargoName)) {
        return $script:CrateSemverVerdictCache[$CargoName]
    }

    $workspacePackage = Get-WorkspacePackages -repoRoot $RepoRoot |
        Where-Object { $_.Folder -eq $Folder -or $_.Name -eq $CargoName } |
        Select-Object -First 1
    if ($null -ne $workspacePackage -and $workspacePackage.IsProcMacroOnly) {
        Write-Host "cargo semver-checks: '$CargoName' is proc-macro-only; manual SemVer review is required." -ForegroundColor Yellow
        $script:CrateSemverVerdictCache[$CargoName] = 'manual'
        return 'manual'
    }

    Write-Host "🔎 cargo semver-checks: analysing '$CargoName' against its previous version-bump commit..." -ForegroundColor Cyan
    $result = Invoke-CrateSemverCheck -PackageName $CargoName -PackageFolder $Folder -RepoRoot $RepoRoot
    $script:CrateSemverVerdictCache[$CargoName] = $result
    return $result
}

# Returns information about all workspace packages as an array of objects with:
#   Name                  - cargo package name
#   Folder                - folder name under crates/ (used as the script's PackageName argument)
#   Published             - $true if the package is published to crates.io
#   Deps                  - array of normalized names of the WORKSPACE MEMBERS this package
#                           depends on (kind 'normal' or 'build', not 'dev'). Membership is
#                           decided by the dependency's resolved path, not its name, so a
#                           registry crate or an out-of-workspace path dependency is excluded
#                           even when it shares a member's package name.
#   DepAliases            - hashtable mapping a normalized dependency name to additional
#                           normalized crate roots observed for it -- a `package = "..."`
#                           alias, or the dependency's own `[lib] name`. An entry does not say
#                           whether a separate unrenamed declaration also exists, so this is
#                           not a complete or exclusive set of reachable roots. Covers the same
#                           workspace-member dependencies as Deps.
#   CrateRoot             - the package's own normalized crate root (its `[lib] name` when it
#                           sets one, else its normalized package name), or $null when the
#                           package has no library target at all. This is the name a crate's
#                           types are written under by anything that does not rename it, so it
#                           is the root an allowlist carries for a re-exported type.
#   AllowedExternalTypes  - array of strings from [package.metadata.cargo_check_external_types],
#                           or $null if the package does not declare them
#   HasLibraryTarget      - $true when cargo metadata reports a regular 'lib' target
#   IsProcMacroOnly       - $true when the package has a 'proc-macro' target and no regular 'lib' target
function Get-WorkspacePackages {
    param([string]$repoRoot)

    $metadata = Get-WorkspaceMetadata -repoRoot $repoRoot
    $cratesDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "crates"))

    $packages = @()

    # A dependency is nameable in Rust source -- and so in an
    # allowed_external_types entry -- by its *crate root*, which is not always
    # its package name: `[lib] name = "..."` renames the crate root while the
    # package keeps its own name. Map each workspace package to its crate root
    # so the dependency loop below can resolve the name an allowlist will
    # actually carry.
    #
    # Only workspace members are covered, because `cargo metadata --no-deps`
    # reports targets for nothing else. That is sufficient here: the exposure
    # cascade only ever asks about workspace packages, since a registry crate
    # is never a release target.
    $crateRootByPackage = @{}
    foreach ($package in $metadata.packages) {
        $libTarget = $package.targets |
            Where-Object { @($_.kind) -contains 'lib' -or @($_.kind) -contains 'proc-macro' } |
            Select-Object -First 1
        if ($null -eq $libTarget) { continue }
        $crateRootByPackage[$package.name.Replace('-', '_')] = ([string]$libTarget.name).Replace('-', '_')
    }

    # Manifest directories of the packages this function returns, used to decide
    # whether a dependency is a workspace edge.
    #
    # Cargo reports a dependency's package name but nothing that distinguishes a
    # workspace member from a registry crate or a path dependency outside the
    # workspace. Every consumer of Deps asks a workspace-reachability question --
    # which crates can carry a released package's types to their own public API --
    # so matching on the text name alone lets an unrelated external crate stand in
    # for a workspace conduit and fabricate a path that does not exist.
    #
    # Identity therefore comes from the resolved path, not the name.
    $memberDirs = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase)
    foreach ($package in $metadata.packages) {
        $memberDir = [System.IO.Path]::GetFullPath((Split-Path $package.manifest_path -Parent))
        if ($memberDir.StartsWith($cratesDir, [System.StringComparison]::OrdinalIgnoreCase)) {
            [void]$memberDirs.Add($memberDir.TrimEnd([System.IO.Path]::DirectorySeparatorChar))
        }
    }

    foreach ($package in $metadata.packages) {
        $manifestDir = [System.IO.Path]::GetFullPath((Split-Path $package.manifest_path -Parent))
        if (-not $manifestDir.StartsWith($cratesDir, [System.StringComparison]::OrdinalIgnoreCase)) {
            continue
        }

        $deps = @()
        $depAliases = @{}
        foreach ($dep in $package.dependencies) {
            if ($dep.kind -eq 'dev') {
                continue
            }

            # A dependency joins the workspace graph only when its `path`
            # resolves to a member directory. `path` is absent for a registry
            # dependency and points outside crates/ for a non-member path
            # dependency; in neither case can the edge carry a workspace
            # package's types, so it must not participate in reachability --
            # nor contribute an alias, which would let an external crate's
            # rename supply an allowlist root for a same-named member.
            $depPathProp = $dep.PSObject.Properties['path']
            if (-not $depPathProp -or [string]::IsNullOrWhiteSpace($depPathProp.Value)) {
                continue
            }
            $depDir = [System.IO.Path]::GetFullPath([string]$depPathProp.Value).
                TrimEnd([System.IO.Path]::DirectorySeparatorChar)
            if (-not $memberDirs.Contains($depDir)) {
                continue
            }

            $depCargoName = $dep.name.Replace('-', '_')
            $deps += $depCargoName

            # `rename` is the `package = "..."` form in Cargo.toml: the crate is
            # declared under one name but reachable in Rust source -- and hence
            # in an allowed_external_types entry -- only under the alias. Record
            # it so Test-PackageExposesTarget can recognize the aliased root.
            $renameProp = $dep.PSObject.Properties['rename']
            if ($renameProp -and -not [string]::IsNullOrWhiteSpace($renameProp.Value)) {
                $alias = ([string]$renameProp.Value).Replace('-', '_')
                # A package may be depended on more than once under different
                # aliases (per-target or per-feature), so collect them all.
                $depAliases[$depCargoName] = @(@($depAliases[$depCargoName]) + $alias |
                        Where-Object { $_ } | Sort-Object -Unique)
            }
            else {
                # No `package = "..."`, so the crate root is whatever the
                # dependency's own manifest calls its lib target. A crate whose
                # `[lib] name` differs from its package name is nameable only
                # under the lib name, so that -- not the package name -- is what
                # an allowlist entry carries.
                #
                # Only reached when `rename` is absent, because `rename` wins:
                # `foo = { package = "bar" }` makes the crate nameable as `foo`
                # regardless of what bar calls its lib target.
                $crateRoot = $crateRootByPackage[$depCargoName]
                if ($crateRoot -and $crateRoot -ne $depCargoName) {
                    $depAliases[$depCargoName] = @(@($depAliases[$depCargoName]) + $crateRoot |
                            Where-Object { $_ } | Sort-Object -Unique)
                }
            }
        }

        $allowedTypes = $null
        $pkgMeta = $package.PSObject.Properties['metadata']
        if ($pkgMeta -and $null -ne $pkgMeta.Value) {
            $externalTypes = $pkgMeta.Value.PSObject.Properties['cargo_check_external_types']
            if ($externalTypes -and $null -ne $externalTypes.Value) {
                $allowed = $externalTypes.Value.PSObject.Properties['allowed_external_types']
                # Only a genuine array is a declared policy. The schema demands
                # one, so any other shape is malformed metadata -- and leaving
                # $allowedTypes as $null routes it to the absent-metadata branch
                # in Test-PackageExposesTarget, which fails closed.
                #
                # Wrapping instead would be a fail-OPEN: `@("std::*")` turns the
                # malformed scalar `allowed_external_types = "std::*"` into a
                # well-formed one-entry allowlist that matches nothing, so the
                # crate reads as provably exposing nothing. `[package.metadata]`
                # is arbitrary TOML that cargo passes through unvalidated, so
                # such a value reaches the planner intact.
                #
                # A string is itself IEnumerable (over its characters), so it
                # must be excluded explicitly or it would read as an array of
                # single-character entries.
                if ($allowed -and $null -ne $allowed.Value -and
                    $allowed.Value -is [System.Collections.IEnumerable] -and
                    $allowed.Value -isnot [string]) {
                    $allowedTypes = @($allowed.Value)
                }
            }
        }

        $targetKinds = @($package.targets | ForEach-Object { @($_.kind) } | Sort-Object -Unique)
        $hasLibraryTarget = $targetKinds -contains 'lib'

        $packages += [pscustomobject]@{
            Name                 = $package.name
            Folder               = Split-Path $manifestDir -Leaf
            Version              = $package.version
            Published            = -not ($null -ne $package.publish -and $package.publish.Count -eq 0)
            Deps                 = $deps
            DepAliases           = $depAliases
            CrateRoot            = $crateRootByPackage[$package.name.Replace('-', '_')]
            AllowedExternalTypes = $allowedTypes
            HasLibraryTarget     = $hasLibraryTarget
            IsProcMacroOnly      = (-not $hasLibraryTarget) -and ($targetKinds -contains 'proc-macro')
        }
    }

    return $packages
}

# Returns the allowlist roots that count as naming $TargetPackageName in the
# dependent's public API: the target's own normalized name, plus any crate root
# it is reachable under.
#
# This is for DECLARED edges only -- it is called solely by
# Test-PackageExposesTarget. A package name is not always the name its types
# are written under, and on a declared edge two things divert it, both already
# recorded in the dependent's DepAliases:
#   - `package = "..."` on the dependency, which makes the crate nameable only
#     under the alias; and
#   - `[lib] name = "..."` in the dependency's own manifest, which renames the
#     crate root for every consumer.
#
# DepAliases is therefore the whole story here, and deliberately so. It is
# authoritative *over the target's global crate root*, because a
# `package = "..."` rename shadows the lib name entirely: a dependent that
# imports the crate as `aliased_dep` cannot write `dep_core::Handle` no matter
# what the target's manifest says. Adding the target's global root back on such
# an edge would re-accept a name the dependent provably cannot use, turning an
# unrelated allowlist entry that happens to collide with it into a false
# exposure and a spurious breaking bump. That is why this function takes no
# crate-root parameter.
#
# A third diversion exists but is not this function's problem: the same
# `[lib] name` carried by an allowlist entry earned on an INDIRECT path. A
# re-exported type is attributed to its defining crate, so a dependent several
# hops away names it under that root -- and having crossed no edge to the
# target, no alias applies to it. Test-PackageAllowlistNamesTarget handles that
# case, building its roots from the target's own record instead. A crate can
# hold both kinds of path at once, so the two are evaluated independently
# rather than as alternatives; see Get-PublishedDependentsExposingTarget.
#
# The real package name below is a known over-acceptance, not a considered
# exception to that rule: a rename shadows the package name exactly as it
# shadows the lib name, so `dependency::*` is equally unwritable on an edge
# reachable only as `aliased_dep`. Narrowing it needs data this record does not
# carry -- whether an *unrenamed* edge to the target also exists, since a
# package may be depended on twice, once aliased and once not. Dropping the
# name without that would fail open on the ordinary unrenamed edge, which is
# the far worse direction, so it stays until the edge data can distinguish the
# two. It errs toward a spurious bump, never toward a missed break.
#
# Any of these roots is one an allowed_external_types entry may carry. Matching
# solely on the real package name would find nothing and report "not exposed"
# -- a fail-open that ships a break as compatible.
function Get-AcceptedExposureRoots {
    param(
        [Parameter(Mandatory = $true)][pscustomobject]$Dependent,
        [Parameter(Mandatory = $true)][string]$TargetPackageName
    )

    $normalizedTarget = $TargetPackageName.Replace('-', '_')

    $roots = @($normalizedTarget)
    if ($Dependent.PSObject.Properties['DepAliases'] -and $null -ne $Dependent.DepAliases) {
        $roots += @($Dependent.DepAliases[$normalizedTarget])
    }

    return @($roots | Where-Object { $_ } | Sort-Object -Unique)
}

# Returns $true unless the package's cargo-check-external-types allowlist is
# positive evidence that its public API cannot name types rooted at the target
# package. Anything short of that evidence counts as exposure: an unknown must
# not permit a breaking dependency bump to ship as a compatible release.
#
# This predicate is for DECLARED edges only, so it deliberately takes no
# TargetCrateRoot: the dependent's own DepAliases entry already records the
# name the target is reachable under on this edge, rename included, and is
# authoritative over the target's global crate root.
function Test-PackageExposesTarget {
    param(
        [Parameter(Mandatory = $true)][pscustomobject]$Dependent,
        [Parameter(Mandatory = $true)][string]$TargetPackageName
    )

    # `$null` here means the crate declares no allowed_external_types policy at
    # all. That is deliberately *not* the same as declaring an empty one:
    # `@()` is a positive claim -- "my public API names nothing foreign" -- so
    # it skips this branch, matches nothing in the loop below, and correctly
    # returns $false. Absent metadata makes no claim at all, so it cannot be
    # read as the stricter of the two. Hence absent => $true, empty => $false.
    #
    # Absent is *nearly* proof of non-exposure anyway: CI runs
    # cargo-check-external-types with an empty allowlist when a crate declares
    # none, so any foreign type in the public API fails the required
    # external-type-exposure job.
    #
    # It is not proof, because CI validates *merged* code while release
    # planning analyses the *working tree* (see Invoke-CrateSemverCheck): an
    # in-progress edit that exposes a dependency's type without adding the
    # allowlist entry has never been checked by anything. A brand-new crate has
    # the same gap until its first CI run. Assume exposure.
    if ($null -eq $Dependent.AllowedExternalTypes) {
        return $true
    }

    $acceptedRoots = Get-AcceptedExposureRoots -Dependent $Dependent `
        -TargetPackageName $TargetPackageName

    foreach ($entry in $Dependent.AllowedExternalTypes) {
        # An entry that is not a usable non-empty string carries no information
        # about what this crate exposes. Skipping it would let the loop fall
        # through to "not exposed" and ship a breaking dependency bump as a
        # compatible release, so treat it the same as absent metadata. (`-split`
        # coerces anything to a string, so a malformed entry does not throw --
        # it silently collapses to '' and matches nothing, which is worse.)
        if ($entry -isnot [string] -or [string]::IsNullOrWhiteSpace($entry)) {
            return $true
        }

        $root = ($entry -split '::', 2)[0]
        if ([string]::IsNullOrWhiteSpace($root)) {
            return $true
        }
        if ($root.Contains('*') -or $root.Contains('?') -or $root.Contains('[')) {
            return $true
        }
        if ($acceptedRoots -contains $root) {
            return $true
        }
    }

    return $false
}

# Returns $true when the dependent's allowlist is positive evidence that its
# public API names types rooted at $TargetPackageName.
#
# This is the affirmative half of Test-PackageExposesTarget with none of its
# fail-closed branches: absent or malformed metadata answers $false here.
# The two are used for different edges, and the difference is deliberate.
#
# Test-PackageExposesTarget answers "may this crate expose the target?" for a
# DIRECT dependency, where absent metadata is a genuine unknown that must fail
# closed.
#
# This function answers "does this crate claim to name the target's types?" for
# an INDIRECT path. cargo-check-external-types attributes a re-exported
# type to its DEFINING crate, so a crate that reaches `a::T` through `b`
# allowlists `a` while depending only on `b` (fetch_azure documents exactly this
# for typespec_client_core). Such a path is invisible to a direct-dependency
# scan, so it needs its own check.
#
# Because the path crosses no edge to the target, the name the allowlist
# carries comes from the target itself -- its crate root -- and never from a
# rename, which only a crate that declares the dependency can apply. Hence
# -TargetCrateRoot. That holds even when the same crate separately declares a
# direct edge: the two paths are judged independently, since a crate can hold
# both and earn a root on one that the other cannot supply.
#
# It must not inherit the fail-closed branches, because "no allowlist" would
# then match every transitive dependency in the graph and force unrelated
# crates breaking. A crate with no allowlist that truly does expose the target
# is still caught: it fails closed on its direct edge to whichever intermediate
# carries the type, and the fixpoint walks that up the graph.
#
# A wildcard root is the one unknown still treated as a match: it can expand to
# the target, and unlike absent metadata it is a deliberate, rare declaration.
function Test-PackageAllowlistNamesTarget {
    param(
        [Parameter(Mandatory = $true)][pscustomobject]$Dependent,
        [Parameter(Mandatory = $true)][string]$TargetPackageName,
        # Matters most on this path: a crate reached indirectly crosses no edge
        # to the target, so no DepAliases entry applies to it. The target's own
        # crate root is the only place the diverted name can come from.
        [string]$TargetCrateRoot
    )

    if ($null -eq $Dependent.AllowedExternalTypes) {
        return $false
    }

    # This predicate judges an INDIRECT path, which crosses no edge to the
    # target. DepAliases is therefore not consulted even when the dependent
    # also declares a direct edge: an alias applies only to the edge that
    # declares it, and a type arriving re-exported through a conduit is
    # attributed to the target's own crate root regardless of what any direct
    # edge renames it to. The direct edge is judged separately, by
    # Test-PackageExposesTarget, against its own aliases.
    #
    # When the target's crate root is known it is exclusive: `[lib] name`
    # replaces the package name as the usable Rust root. The package name
    # remains only as compatibility for older synthetic records that predate
    # CrateRoot.
    if (-not [string]::IsNullOrWhiteSpace($TargetCrateRoot)) {
        $acceptedRoots = @($TargetCrateRoot.Replace('-', '_'))
    } else {
        $acceptedRoots = @($TargetPackageName.Replace('-', '_'))
    }

    foreach ($entry in $Dependent.AllowedExternalTypes) {
        if ($entry -isnot [string] -or [string]::IsNullOrWhiteSpace($entry)) {
            continue
        }

        $root = ($entry -split '::', 2)[0]
        if ($root.Contains('*') -or $root.Contains('?') -or $root.Contains('[')) {
            return $true
        }
        if ($acceptedRoots -contains $root) {
            return $true
        }
    }

    return $false
}

# Returns the Cargo variable that overrides the linker for the host target
# (e.g. CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER), or $null when the
# toolchain default should stand.
#
# MSVC link.exe is not long-path aware, so baseline builds under a deep target
# directory fail with LNK1104; rust-lld handles those paths. Only *-msvc hosts
# need this -- other targets already default to long-path-safe linkers, and on
# *-windows-gnu rust-lld would stand in for the gcc driver and invoke lld
# directly, a different link path this fix has no reason to disturb.
function Get-SemverChecksLinkerEnvName {
    [CmdletBinding()]
    param()

    if (-not $IsWindows) {
        return $null
    }

    # A missing or failing rustc yields no usable output, which the match below
    # already rejects; $LASTEXITCODE would be no help here anyway, since it is
    # only updated for native executables and would hold a stale value whenever
    # rustc resolves to a shim.
    $version = try { & rustc -vV 2>$null | Out-String } catch { '' }

    # Read the capture off the Match object rather than $Matches, which -match
    # leaves untouched when it fails and so can hold a stale value.
    $hostMatch = [regex]::Match($version, '(?m)^host:\s*(\S+)')
    if (-not $hostMatch.Success) {
        # Fail open -- a probe must never break a release. Say so, though:
        # otherwise the LNK1104 this override exists to prevent comes back with
        # nothing to suggest the remedy simply never ran.
        Write-Warning 'Could not read the host triple from `rustc -vV`; leaving the toolchain default linker in place. Baseline builds under a long path may fail with LNK1104.'
        return $null
    }

    $hostTriple = $hostMatch.Groups[1].Value
    if ($hostTriple -notmatch '(?i)-msvc$') {
        return $null
    }

    $triple = $hostTriple.ToUpperInvariant() -replace '[^A-Z0-9]', '_'
    return "CARGO_TARGET_${triple}_LINKER"
}

# Directory name for relocated semver-checks builds, placed at a volume root.
# Kept terse on purpose: every character here is one the MAX_PATH budget loses.
$script:SemverChecksTargetDirName = 'oxi-sc'

# Returns a short, per-clone target directory for baseline builds on Windows,
# or $null where the default target directory should stand.
#
# The linker override above keeps link.exe out of the way, but it cannot help
# the C compilers that -sys crates drive through the cc crate. MSVC cl.exe
# resolves its -Fo argument against MAX_PATH and fails with C1083 ("Cannot open
# compiler generated file"), and unlike the linker there is no long-path-aware
# drop-in to switch to: clang-cl is not guaranteed to be installed. The
# remaining lever is the length of the path itself.
#
# cargo-semver-checks nests baseline builds under the workspace target
# directory and offers no flag to move them, but it derives that location from
# cargo metadata, so CARGO_TARGET_DIR does reach it. Rooting the build at the
# repository's own volume keeps it on the filesystem the developer chose, and
# the digest of the repository root keeps sibling clones from sharing one
# directory. The result is a fixed 18 characters, in place of a repository path
# that is unbounded.
#
# The path is deterministic rather than unique so that consecutive runs reuse
# the baseline rustdoc they just built; concurrent runs are safe because cargo
# locks the target directory exactly as it does for target/.
function Get-SemverChecksTargetDirPath {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    if (-not $IsWindows) {
        return $null
    }

    # Normalise first: the digest has to be stable across callers that spell the
    # same repository root differently (trailing separator, relative segments,
    # or casing, none of which Windows treats as distinct).
    $full = [System.IO.Path]::GetFullPath($RepoRoot).TrimEnd('\')

    # A UNC root has no drive letter to anchor to and would keep the very
    # length this function exists to shed, so fall back to the system drive.
    $volume = [System.IO.Path]::GetPathRoot($full)
    if ([string]::IsNullOrWhiteSpace($volume) -or $volume.StartsWith('\\')) {
        $volume = "$env:SystemDrive\"
    }

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $digest = $sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($full.ToLowerInvariant()))
    } finally {
        $sha.Dispose()
    }
    $token = [System.BitConverter]::ToString($digest[0..3]).Replace('-', '').ToLowerInvariant()

    return (Join-Path $volume (Join-Path $script:SemverChecksTargetDirName $token))
}

# Runs cargo semver-checks, linking with rust-lld and building under a short
# target directory where MSVC tooling would otherwise overflow MAX_PATH.
#
# The setting travels by environment variable because that is the only channel
# that reaches the cargo invocation which matters. cargo-semver-checks exposes
# no flag to forward cargo configuration (checked against 0.47.0), and as an
# external subcommand it receives `--config` itself rather than letting cargo
# interpret it -- while the build that overflows MAX_PATH is the one the tool
# spawns internally.
#
# Two details keep the rest short. The bare linker name is enough: rustc puts
# its own linker directory on PATH when it invokes the linker, so rust-lld
# resolves without locating the sysroot. And overriding only the linker leaves
# the repository's rustflags (such as -C target-cpu) intact, which setting
# RUSTFLAGS would not -- that replaces target.<triple>.rustflags wholesale.
function Invoke-SemverChecksCli {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$PackageName,
        [Parameter(Mandatory = $true)][string]$BaselineSha,
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    Push-Location $RepoRoot
    try {
        # Probe from inside the repository. rustup resolves toolchain overrides
        # against the working directory, so a probe run elsewhere can name a
        # variable for a different host triple -- one cargo then ignores,
        # leaving the baseline to link with the default linker after all.
        $linkerVar = Get-SemverChecksLinkerEnvName
        $applied = $false

        if ($linkerVar) {
            $path = "Env:\$linkerVar"
            if (Test-Path $path) {
                # An explicit choice wins. The premise for overriding is that
                # link.exe cannot handle long paths; whoever set this variable
                # has already steered cargo away from link.exe.
                Write-Verbose "$linkerVar is already set; leaving the configured linker in place."
            } else {
                Set-Item -Path $path -Value 'rust-lld.exe'
                $applied = $true
            }
        }

        $targetDirApplied = $false
        $targetDir = Get-SemverChecksTargetDirPath -RepoRoot $RepoRoot
        if ($targetDir) {
            if (Test-Path 'Env:\CARGO_TARGET_DIR') {
                # As with the linker, an explicit choice wins: whoever set this
                # has already decided where the build should land.
                Write-Verbose 'CARGO_TARGET_DIR is already set; building where it points.'
            } else {
                try {
                    $null = New-Item -ItemType Directory -Path $targetDir -Force -ErrorAction Stop
                    $env:CARGO_TARGET_DIR = $targetDir
                    $targetDirApplied = $true
                } catch {
                    # Fail open -- a probe must never break a release. Name the
                    # symptom, though, so a later C1083 is not a mystery.
                    Write-Warning "Could not create '$targetDir'; building under the default target directory. If the repository path is long, the baseline build may fail with C1083. ($($_.Exception.Message))"
                }
            }
        }

        try {
            # A required version bump produces an expected non-zero exit code.
            $PSNativeCommandUseErrorActionPreference = $false
            $output = & cargo semver-checks --package $PackageName --baseline-rev $BaselineSha --all-features --color never 2>&1 | Out-String
            $exitCode = $LASTEXITCODE
        } finally {
            if ($applied) {
                Remove-Item -Path "Env:\$linkerVar" -ErrorAction SilentlyContinue
            }
            if ($targetDirApplied) {
                Remove-Item -Path 'Env:\CARGO_TARGET_DIR' -ErrorAction SilentlyContinue
            }
        }
    } finally {
        Pop-Location
    }

    return [pscustomobject]@{
        Output   = $output
        ExitCode = $exitCode
    }
}

# Runs `cargo semver-checks` for a single crate against its previous version-bump
# commit in git history. The baseline commit is located with
# Get-PreviousVersionBumpCommit and passed to cargo-semver-checks as
# `--baseline-rev <sha>`, which rebuilds the baseline rustdoc from the crate's
# source at that commit — so the comparison source is what the repository last
# *declared*, with no registry access. This works identically in OSS and
# enterprise/offline environments and treats a declared-but-unpublished version as
# the baseline (unlike the former registry lookup).
#
# $BaseRef selects which bump counts as "previous"; the planner uses HEAD (the
# last committed version bump = the previous release). Returns the minimum change
# type the current working-tree API requires: 'breaking', 'non-breaking', 'patch',
# or 'none' when there is no prior version-bump commit to compare against (a
# brand-new crate).
#
# The current API is analysed from the working tree, not from HEAD, so a
# coordinated release's in-progress source edits are reflected rather than only
# what has been committed.
#
# That is necessary but NOT sufficient for exposed-dependency breaks, and this
# function must not be read as covering them. When a dependency's version bump
# is incompatible without its type *shapes* changing, this crate's rustdoc is
# identical on both sides of the comparison, so semver-checks correctly reports
# no required bump — yet releasing this crate compatibly is still wrong, because
# type identity in Rust is per-version: a consumer cannot hand a `dep 0.7` type
# to an API expecting `dep 0.8`. Nothing in a rustdoc diff can show that.
#
# Exposure is therefore decided separately, from the crate's declared
# allowed_external_types (Test-PackageExposesTarget), and propagated to a
# fixpoint by Resolve-ReleaseSet. The two are complementary: semver-checks
# supplies each crate's own floor, the exposure cascade supplies the floor its
# dependencies impose on it.
function Invoke-CrateSemverCheck {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$PackageName,
        [Parameter(Mandatory = $true)][string]$PackageFolder,
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [string]$BaseRef = 'HEAD'
    )

    # Locate the previous version-bump commit. No such commit => brand-new crate:
    # nothing to compare against, so it imposes no change-type floor.
    $bump = Get-PreviousVersionBumpCommit -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $PackageFolder
    if ($null -eq $bump) {
        return 'none'
    }

    $result = Invoke-SemverChecksCli -PackageName $PackageName -BaselineSha $bump.Sha -RepoRoot $RepoRoot

    return ConvertFrom-SemverChecksOutput -Output $result.Output -ExitCode $result.ExitCode -PackageName $PackageName
}

# Parses `cargo semver-checks` combined output into a change type. Pure (no I/O)
# so it can be unit-tested against captured tool output, with one caveat: the
# failure message carries a platform-conditional path-length hint, so it is a
# function of the arguments *and* $IsWindows. With the git-history
# baseline (`--baseline-rev`) cargo-semver-checks always builds the baseline from
# source, so the only outcomes are a semver verdict or a genuine tool/build
# failure. Mapping:
#   * "N major and M minor checks failed" -> major>0 breaking; minor>0 non-breaking
#   * "no semver update required"                -> patch (compatible)
#   * anything else (tool/build failure)         -> throw (no silent fallback)
# The 'none' (no baseline) case is decided by the CALLER before this function is
# invoked — when there is no previous version-bump commit to compare against —
# not from tool output here.
function ConvertFrom-SemverChecksOutput {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Output,
        [int]$ExitCode = 0,
        [string]$PackageName = ''
    )

    $m = [regex]::Match($Output, '(?i)(\d+)\s+major\s+and\s+(\d+)\s+minor\s+check')
    if ($m.Success) {
        if ([int]$m.Groups[1].Value -gt 0) { return 'breaking' }
        if ([int]$m.Groups[2].Value -gt 0) { return 'non-breaking' }
        return 'patch'
    }

    if ($Output -match '(?i)no\s+semver\s+update\s+required') {
        return 'patch'
    }

    $pathHint = if ($IsWindows) {
        ' If the output contains LNK1104, C1083 or a path-length error, a MAX_PATH-bound tool was reached despite the short build directory these scripts select; setting CARGO_TARGET_DIR to a shorter path, or moving the repository closer to the volume root, buys back the remaining characters.'
    } else {
        ''
    }
    throw "cargo semver-checks did not produce a parseable result for '$PackageName' (exit $ExitCode). This usually means the tool is missing or the crate/baseline failed to build.$pathHint Output:`n$Output"
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

# Returns the published workspace packages that directly depend on a cargo
# package in an already-captured metadata snapshot. This deliberately follows
# exactly one dependency edge; callers can advance a review frontier only after
# classifying that edge's consumer.
function Get-DirectPublishedDependentsFromBaseline {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][object[]]$Baseline,
        [Parameter(Mandatory = $true)][string]$TargetCargoName
    )

    return @(
        $Baseline |
            Where-Object {
                $_.Published -and
                $_.Name.Replace('-', '_') -ne $TargetCargoName -and
                $_.Deps -contains $TargetCargoName
            } |
            Sort-Object -Property Folder |
            ForEach-Object { $_.Folder }
    )
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
# version diffing. Used as a test utility to synthesise a release set from a
# synthetic-workspace git diff without having to construct one entry-by-entry;
# production code uses Resolve-ReleaseSet in release-flow.ps1 (driven by
# explicit user input).
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
# A BFS root only counts as "released" for this analysis when the release-set
# member itself has source modifications past its release baseline (i.e. is
# in the modifications map). A pure-cascade member (version bump only, no
# source changes of its own) cannot have started consuming unreleased
# features in its dependencies because nothing in its source changed — BFS
# from such a member would only produce false positives, so it is skipped.
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
#   PlannedCurrentVersion    - release plan's starting version, or $null when
#                              the package is not yet in the release set
#   EffectiveChangeType      - release level already in the plan, or $null
#   EffectiveTargetVersion   - target version already in the plan, or $null
#   ChangedFileCount  - number of files changed under crates/<folder>/ since baseline
#   DependencyChains  - @( @('released_package', 'mid_package', 'this_dep'), ... )
#                       - chains rooted in release-set members (or, in
#                       -IncludeAllModifiedAsRoots mode, also in other
#                       modified-published packages) that transitively reach
#                       `this_dep`. Used by the interactive review prompt to
#                       highlight what is at risk in the current release plan
#                       specifically.
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
    # release-set members WHOSE SOURCE/FILES HAVE BEEN MODIFIED past their
    # per-package release baseline. Pure-cascade members (no source changes
    # of their own, version bump only) cannot have started consuming
    # unreleased features in their dependencies, so BFS from them is
    # categorically incapable of producing a real finding — only false
    # positives — and is skipped. The same modified-precondition applies in
    # -IncludeAllModifiedAsRoots mode, where it's redundant with the
    # modifiedMap union below (release-set membership adds nothing once
    # modified-published already covers it) but kept for symmetry / clarity.
    # When -IncludeAllModifiedAsRoots is set we also add every
    # modified-published package so chains between changed packages can be
    # recorded (e.g. 'bytesbuf_io -> bytesbuf' when both are changed and
    # bytesbuf_io depends on bytesbuf). Sorted for deterministic prompt order.
    $rootFolders = if ($IncludeAllModifiedAsRoots) {
        $set = [System.Collections.Generic.HashSet[string]]::new()
        foreach ($k in $ResolvedReleaseSet.Keys) {
            if ($modifiedMap.ContainsKey($k)) { [void]$set.Add($k) }
        }
        foreach ($k in $modifiedMap.Keys) {
            $pkg = $byFolder[$k]
            if ($null -ne $pkg -and $pkg.Published) { [void]$set.Add($k) }
        }
        @($set | Sort-Object)
    } else {
        @($ResolvedReleaseSet.Keys | Where-Object { $modifiedMap.ContainsKey($_) } | Sort-Object)
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
                            Folder                     = $depFolder
                            PackageName                = $depPackage.Name
                            CurrentVersion             = $depPackage.Version
                            InReleaseSet               = $isInReleaseSet
                            PlannedCurrentVersion      = if ($isInReleaseSet) { $depEntry.CurrentVersion } else { $null }
                            EffectiveChangeType        = if ($isInReleaseSet) { $depEntry.EffectiveChangeType } else { $null }
                            EffectiveTargetVersion     = if ($isInReleaseSet) { $depEntry.EffectiveTargetVersion } else { $null }
                            ChangedFileCount           = $modifiedMap[$depFolder]
                            DependencyChains           = @(, $depChain)
                            RequiresManualSemverReview = [bool]$depPackage.IsProcMacroOnly
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

    # Phase B sweep: every BFS root the surfacing predicate accepts but no
    # BFS run reached as a dep gets added as a stub finding (empty chains).
    # Two reasons this matters:
    #
    #   1. -IncludeAllModifiedAsRoots mode: every modified-published package
    #      that isn't BFS-reachable from another root surfaces as a stub.
    #      Renders as "No dependents in release set" in the menu — the
    #      "imaginary `*` package depends on every changed package" UX
    #      without introducing a sentinel.
    #
    #   2. Targeted mode (Invariant B — release-set elevation review):
    #      release-set members that are themselves modified BUT whose
    #      cascade-applied change type is below "breaking" need to surface
    #      for elevation review. With the LIVE filter applied to BFS root
    #      selection, only release-set members IN modifiedMap are roots,
    #      so this sweep over rootFolders is exactly the set of candidates
    #      that qualify for Invariant B. The shouldSurface predicate
    #      filters out user-source members and breaking-cascade members,
    #      leaving only cascade-source below-breaking entries — i.e.
    #      release-set members the user may want to elevate after diff
    #      review.
    foreach ($folder in $rootFolders) {
        if ($findings.Contains($folder)) { continue }
        if (-not (& $shouldSurface $folder)) { continue }
        $pkg = $byFolder[$folder]
        $entry = $ResolvedReleaseSet[$folder]
        $findings[$folder] = [pscustomobject]@{
            Folder                     = $folder
            PackageName                = $pkg.Name
            CurrentVersion             = $pkg.Version
            InReleaseSet               = $null -ne $entry
            PlannedCurrentVersion      = if ($null -ne $entry) { $entry.CurrentVersion } else { $null }
            EffectiveChangeType        = if ($null -ne $entry) { $entry.EffectiveChangeType } else { $null }
            EffectiveTargetVersion     = if ($null -ne $entry) { $entry.EffectiveTargetVersion } else { $null }
            ChangedFileCount           = $modifiedMap[$folder]
            DependencyChains           = @()
            RequiresManualSemverReview = [bool]$pkg.IsProcMacroOnly
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
