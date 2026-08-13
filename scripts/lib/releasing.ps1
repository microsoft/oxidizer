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
#     type for each released package is supplied by the release skill
#     (e.g. `mypkg@breaking`, `mypkg@nonbreaking`).
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
# user-visible output without translating `non-breaking` to `nonbreaking`.
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
# These are valid for the whole release-skill invocation because:
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

# Returns information about all workspace packages as an array of objects with:
#   Name                  - cargo package name
#   Folder                - folder name under crates/ (used as the script's PackageName argument)
#   Published             - $true if the package is published to crates.io
#   Deps                  - array of normalized dependency names (kind 'normal' or 'build', not 'dev')
#   DepAliases            - hashtable mapping a normalized dependency name to additional
#                           normalized crate roots observed for it -- a `package = "..."`
#                           alias, or the dependency's own `[lib] name`. An entry does not say
#                           whether a separate unrenamed declaration also exists, so this is
#                           not a complete or exclusive set of reachable roots.
#   CrateRoot             - the package's own normalized crate root (its `[lib] name` when it
#                           sets one, else its normalized package name), or $null when the
#                           package has no library target at all. This is the name a crate's
#                           types are written under by anything that does not rename it, so it
#                           is the root an allowlist carries for a re-exported type.
#   AllowedExternalTypes  - array from [package.metadata.cargo_check_external_types]
#   ExposureMetadataKnown - $true when allowed_external_types is explicitly present
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

        $allowedExternalTypes = $null
        $exposureMetadataKnown = $false
        $packageMetadata = $package.PSObject.Properties['metadata']
        if ($packageMetadata -and $null -ne $packageMetadata.Value) {
            $externalTypesMetadata = $packageMetadata.Value.PSObject.Properties['cargo_check_external_types']
            if ($externalTypesMetadata -and $null -ne $externalTypesMetadata.Value) {
                $allowedTypes = $externalTypesMetadata.Value.PSObject.Properties['allowed_external_types']
                if ($allowedTypes -and $null -ne $allowedTypes.Value) {
                    $allowedExternalTypes = @($allowedTypes.Value)
                    $exposureMetadataKnown = $true
                }
            }
        }

        $targetKinds = @($package.targets | ForEach-Object { @($_.kind) } | Sort-Object -Unique)
        $hasLibraryTarget = $targetKinds -contains 'lib'

        $packages += [pscustomobject]@{
            Name                    = $package.name
            Folder                  = Split-Path $manifestDir -Leaf
            Version                 = $package.version
            Published               = -not ($null -ne $package.publish -and $package.publish.Count -eq 0)
            Deps                    = @($deps | Sort-Object -Unique)
            DepAliases              = $depAliases
            CrateRoot               = $crateRootByPackage[$package.name.Replace('-', '_')]
            AllowedExternalTypes    = $allowedExternalTypes
            ExposureMetadataKnown   = $exposureMetadataKnown
            HasLibraryTarget        = $hasLibraryTarget
            IsProcMacroOnly         = (-not $hasLibraryTarget) -and ($targetKinds -contains 'proc-macro')
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
# `[lib] name` seen from a crate that does NOT declare the edge. A re-exported
# type is attributed to its defining crate, so a dependent several hops away
# names it without depending on it -- and having no edge, it has no DepAliases
# entry for it either. Test-PackageAllowlistNamesTarget handles that case,
# building its roots from the target's own record instead.
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
# an INDIRECT dependency. cargo-check-external-types attributes a re-exported
# type to its DEFINING crate, so a crate that reaches `a::T` through `b`
# allowlists `a` while depending only on `b` (fetch_azure documents exactly this
# for typespec_client_core). Such an edge is invisible to a direct-dependency
# scan, so it needs its own check.
#
# Because there is no declared edge, the name the allowlist carries comes from
# the target itself -- its crate root -- and never from a rename, which only a
# crate that declares the dependency can apply. Hence -TargetCrateRoot.
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
        # Matters most on this path: a crate reached indirectly declares no edge
        # to the target, so it has no DepAliases entry for it. The target's own
        # crate root is the only place the diverted name can come from.
        [string]$TargetCrateRoot
    )

    if ($null -eq $Dependent.AllowedExternalTypes) {
        return $false
    }

    # This predicate is called only when no dependency edge to the target
    # exists. DepAliases is therefore irrelevant: production can populate an
    # alias only while processing a declared edge, which would take the direct
    # branch instead. When the target's crate root is known it is exclusive:
    # `[lib] name` replaces the package name as the usable Rust root. The
    # package name remains only as compatibility for older synthetic records
    # that predate CrateRoot.
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

# Parses `cargo semver-checks` combined output into a change type. Pure (no I/O)
# so it can be unit-tested against captured tool output. With the git-history
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

    throw "cargo semver-checks did not produce a parseable result for '$PackageName' (exit $ExitCode). This usually means the tool is missing or the crate/baseline failed to build. Output:`n$Output"
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
