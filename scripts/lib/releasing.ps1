# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Shared helpers for crate-release tooling. Dot-source from other scripts; never run directly.

.DESCRIPTION
    This file is a library, not an entrypoint. It is loaded into the caller's scope via
    dot-sourcing, e.g.

        . "$PSScriptRoot/lib/releasing.ps1"

    It exposes functions for:
      - Workspace metadata access (cached via `cargo metadata`).
      - Reverse-dependency cascade computation.
      - SemVer arithmetic (Cargo's 0.x.y rules).
      - Safe git invocation (no Invoke-Expression).
      - Detecting which crates have been bumped in this PR, which have had source
        modifications since their own last release baseline (per-crate, derived from
        each crate's Cargo.toml history), and which workspace dependencies of
        in-release crates fall into the "modified-but-unreleased" bucket (the core
        "unreleased upstream dependency" analysis).

    It has no top-level param() block and no side effects beyond declaring script-scope
    caches & compiled regexes.
#>

# --- COMPILED REGEX PATTERNS ---

$script:ConventionalCommitRegex = [regex]'^(\w+)(?:\(.*\))?(!)?:\s*(.*)'
$script:PrReferenceRegex = [regex]'\s*(\(#(\d+)\))$'
$script:SemanticVersionRegex = [regex]'^\d+\.\d+\.\d+$'
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

function Test-ValidCrateName {
    param([string]$crateName)
    return $crateName -match '^[a-zA-Z0-9]([a-zA-Z0-9_-]*[a-zA-Z0-9])?$' -and $crateName.Length -le 64
}

function Test-ValidVersion {
    param([string]$version)
    if ([string]::IsNullOrEmpty($version)) {
        return $true
    }
    return $script:SemanticVersionRegex.IsMatch($version)
}

# Returns -1, 0, or 1 — semantic version ordering on the (major, minor, patch) triple.
function Compare-SemanticVersions {
    param(
        [string]$version1,
        [string]$version2
    )

    # Force array context — a single-segment input ('1') pipes a scalar [int] out
    # of ForEach-Object, and `$x += 0` on a scalar [int] performs arithmetic
    # rather than array concatenation, so the pad-to-3 loop below would never
    # terminate.
    $v1Parts = @($version1.Split('.') | ForEach-Object { [int]$_ })
    $v2Parts = @($version2.Split('.') | ForEach-Object { [int]$_ })

    while ($v1Parts.Count -lt 3) { $v1Parts += 0 }
    while ($v2Parts.Count -lt 3) { $v2Parts += 0 }

    for ($i = 0; $i -lt 3; $i++) {
        if ($v1Parts[$i] -gt $v2Parts[$i]) { return 1 }
        elseif ($v1Parts[$i] -lt $v2Parts[$i]) { return -1 }
    }

    return 0
}

# Computes the next version for the given bump kind, honoring Cargo's 0.x.y SemVer rules:
#   - For x.y.z (x >= 1): major -> (x+1).0.0, minor -> x.(y+1).0, patch -> x.y.(z+1)
#   - For 0.x.y (x >= 1): major -> 0.(x+1).0, minor and patch -> 0.x.(y+1)
#   - For 0.0.x          : every bump -> 0.0.(x+1) (every change is breaking)
function Get-NextVersion {
    param(
        [string]$currentVersion,
        [ValidateSet('major', 'minor', 'patch')]
        [string]$bump
    )

    # Force array context — see Compare-SemanticVersions for the rationale.
    $parts = @($currentVersion.Split('.') | ForEach-Object { [int]$_ })
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

function Get-BumpKindFromVersions {
    param(
        [string]$oldVersion,
        [string]$newVersion
    )

    # Force array context — see Compare-SemanticVersions for the rationale.
    $oldParts = @($oldVersion.Split('.') | ForEach-Object { [int]$_ })
    $newParts = @($newVersion.Split('.') | ForEach-Object { [int]$_ })
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

function Test-IsBreakingChange {
    param(
        [string]$oldVersion,
        [ValidateSet('major', 'minor', 'patch')]
        [string]$bump
    )

    # Force array context — see Compare-SemanticVersions for the rationale.
    $parts = @($oldVersion.Split('.') | ForEach-Object { [int]$_ })
    while ($parts.Count -lt 3) { $parts += 0 }

    if ($parts[0] -ge 1) {
        return $bump -eq 'major'
    }
    if ($parts[1] -ge 1) {
        return $bump -eq 'major'
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

# Reads the [package] `version = "..."` from a crate's Cargo.toml at $BaseRef.
# Returns $null if the file does not exist at that ref (e.g. crate added in this PR).
#
# Cached for the lifetime of the script run: $BaseRef is fixed by the caller
# for the entire run and the script never makes git commits, so the result
# for a given (BaseRef, CrateFolder) pair is invariant. Saves N×`git show`
# spawns per `Invoke-PostReleaseDepScan` loop iteration (the dominant cost
# of the "Analyzing packages..." pause on Windows).
function Get-CrateVersionFromRef {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef,
        [Parameter(Mandatory = $true)][string]$CrateFolder
    )

    if ($null -eq $script:CrateVersionAtRefCache) {
        $script:CrateVersionAtRefCache = @{}
    }
    $cacheKey = "$RepoRoot`u{2402}$BaseRef`u{2402}$CrateFolder"
    if ($script:CrateVersionAtRefCache.ContainsKey($cacheKey)) {
        return $script:CrateVersionAtRefCache[$cacheKey]
    }

    $output = Invoke-Git -Arguments @('show', "${BaseRef}:crates/$CrateFolder/Cargo.toml") -RepoRoot $RepoRoot -AllowFailure
    $result = $null
    if ($null -ne $output) {
        $content = ($output -join "`n")
        $m = $script:CargoPackageVersionRegex.Match($content)
        if ($m.Success) { $result = $m.Groups[2].Value }
    }

    $script:CrateVersionAtRefCache[$cacheKey] = $result
    return $result
}

# --- WORKSPACE METADATA ---

# Cached `cargo metadata --no-deps` for the workspace. Graph topology is safe to cache
# across nested release runs; mutable version data is read fresh from disk via
# Get-CurrentVersion to avoid staleness.
$script:CachedWorkspaceMetadata = $null

# Caches for git-derived data that is invariant for the entire script run.
# These are valid for the whole release-crate.ps1 invocation because:
#   - $BaseRef is fixed by the caller for the entire run, and
#   - the script never makes git commits (HEAD does not move).
# Therefore the per-crate baseline commit, the per-crate committed-changes
# diff, and the per-crate version-at-BaseRef are all stable for the whole
# session. They are populated lazily (first hit) and cleared only by
# Reset-ReleaseScriptCaches — NOT by the routine, mid-flow
# Invalidate-WorkspaceMetadataCache calls that the cascade fires after each
# in-memory Cargo.toml edit (those edits change cargo metadata's view of
# on-disk versions but leave git history untouched).
$script:CrateLastReleaseBaselineCache = $null
$script:CrateCommittedChangesCache    = $null
$script:CrateVersionAtRefCache        = $null

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
# (CrateLastReleaseBaselineCache, CrateCommittedChangesCache,
# CrateVersionAtRefCache) — those are keyed on git history, which the
# release script never mutates (no commits are made). Test isolation
# between scenarios should call Reset-ReleaseScriptCaches instead, which
# clears every cache including this one.
function Invalidate-WorkspaceMetadataCache {
    $script:CachedWorkspaceMetadata = $null
}

# Clears every script-scoped cache used by the release tooling: workspace
# metadata AND the git-derived per-crate caches (baseline commit, committed
# changes, version-at-BaseRef). Intended for test isolation between
# scenarios that build distinct synthetic workspaces — production code uses
# Invalidate-WorkspaceMetadataCache for the routine mid-flow invalidation
# after Cargo.toml edits.
function Reset-ReleaseScriptCaches {
    $script:CachedWorkspaceMetadata       = $null
    $script:CrateLastReleaseBaselineCache = $null
    $script:CrateCommittedChangesCache    = $null
    $script:CrateVersionAtRefCache        = $null
}

# Returns information about all workspace crates as an array of objects with:
#   Name                  - cargo package name
#   Folder                - folder name under crates/ (used as the script's CrateName argument)
#   Published             - $true if the crate is published to crates.io
#   Deps                  - array of normalized dependency names (kind 'normal' or 'build', not 'dev')
#   AllowedExternalTypes  - array of strings from [package.metadata.cargo_check_external_types],
#                           or $null if the crate does not declare them.
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

        $crates += [pscustomobject]@{
            Name                 = $package.name
            Folder               = Split-Path $manifestDir -Leaf
            Version              = $package.version
            Published            = -not ($null -ne $package.publish -and $package.publish.Count -eq 0)
            Deps                 = $deps
            AllowedExternalTypes = $allowedTypes
        }
    }

    return $crates
}

# Returns $true if the dependent crate exposes any type rooted at the target crate
# in its public API, as declared by [package.metadata.cargo_check_external_types].
# Conservative when metadata is missing.
function Test-CrateExposesTarget {
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
# workspace crates that depend on the given target (transitively) via [dependencies]
# or [build-dependencies]. The target itself is not included.
function Get-AllTransitiveDependents {
    param(
        [string]$crateName,
        [string]$repoRoot
    )

    $crates = Get-WorkspaceCrates -repoRoot $repoRoot

    $targetCrate = $crates | Where-Object { $_.Folder -eq $crateName -or $_.Name -eq $crateName } | Select-Object -First 1
    if ($null -eq $targetCrate) {
        Write-Warning "Crate '$crateName' not found in workspace metadata; cannot compute dependents."
        return @()
    }
    $normalizedTarget = $targetCrate.Name.Replace('-', '_')

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

    return $dependents
}

# --- FILE-CHANGE ANALYSIS ---

# Returns the crate folder name (under crates/) that contains the given repo-relative
# path, or $null if the path is outside any crate.
function Get-CrateFolderForPath {
    param([string]$Path)

    $normalized = $Path.Replace('\', '/')
    if (-not $normalized.StartsWith('crates/')) { return $null }
    $rest = $normalized.Substring('crates/'.Length)
    $slash = $rest.IndexOf('/')
    if ($slash -le 0) { return $null }
    return $rest.Substring(0, $slash)
}

# Returns the SHA of the most recent commit that touched the `version =` or
# `publish =` line in the crate's Cargo.toml, or $null if no such commit exists
# in the crate's committed history. This is the per-crate "last release boundary":
# any change under crates/<folder>/ newer than this commit is unreleased from the
# perspective of crates.io, regardless of which PR introduced it.
#
# We intentionally do not rely on git tags. The repo creates them after merge to
# main, but a CI-time clone or a partial fetch may not have them, and a tag is
# downstream evidence of a release while the Cargo.toml edit is the cause.
#
# Cached for the lifetime of the script run (the script never commits, so the
# baseline SHA per folder is invariant). The cache is cleared by
# Reset-ReleaseScriptCaches between test scenarios; production mid-flow
# invalidations (Invalidate-WorkspaceMetadataCache) deliberately leave it alone.
function Get-CrateLastReleaseBaseline {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$CrateFolder
    )

    if ($null -eq $script:CrateLastReleaseBaselineCache) {
        $script:CrateLastReleaseBaselineCache = @{}
    }
    $cacheKey = "$RepoRoot`u{2402}$CrateFolder"
    if ($script:CrateLastReleaseBaselineCache.ContainsKey($cacheKey)) {
        return $script:CrateLastReleaseBaselineCache[$cacheKey]
    }

    $relPath = "crates/$CrateFolder/Cargo.toml"
    # -G matches any added/removed diff line whose content matches the regex.
    # Anchoring at column 0 keeps us on top-level keys, not version-like strings
    # appearing inside dependency tables or arbitrary literals.
    $out = Invoke-Git -Arguments @('log', '-1', '--format=%H', '-G', '^(version|publish)\s*=', '--', $relPath) -RepoRoot $RepoRoot -AllowFailure
    $result = $null
    if ($null -ne $out) {
        $sha = (@($out))[0]
        if (-not [string]::IsNullOrWhiteSpace($sha)) {
            $result = $sha.ToString().Trim()
        }
    }

    $script:CrateLastReleaseBaselineCache[$cacheKey] = $result
    return $result
}

# Returns the list of repo-relative paths under crates/<CrateFolder>/ that
# have changed in committed history between the crate's last release baseline
# (see Get-CrateLastReleaseBaseline) and HEAD. Returns an empty array if the
# crate has no prior release boundary recorded.
#
# Cached for the lifetime of the script run (the script never commits, so the
# committed diff per folder is invariant). The cache is cleared by
# Reset-ReleaseScriptCaches between test scenarios.
function Get-CrateCommittedChanges {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$CrateFolder
    )

    if ($null -eq $script:CrateCommittedChangesCache) {
        $script:CrateCommittedChangesCache = @{}
    }
    $cacheKey = "$RepoRoot`u{2402}$CrateFolder"
    if ($script:CrateCommittedChangesCache.ContainsKey($cacheKey)) {
        return $script:CrateCommittedChangesCache[$cacheKey]
    }

    $baseline = Get-CrateLastReleaseBaseline -RepoRoot $RepoRoot -CrateFolder $CrateFolder
    $paths = New-Object 'System.Collections.Generic.List[string]'
    if (-not [string]::IsNullOrEmpty($baseline)) {
        $committed = Invoke-Git -Arguments @('diff', '--name-only', $baseline, 'HEAD', '--', "crates/$CrateFolder") -RepoRoot $RepoRoot
        foreach ($line in $committed) {
            $p = $line.ToString().Trim().Replace('\', '/')
            if (-not [string]::IsNullOrEmpty($p)) { $paths.Add($p) }
        }
    }
    $result = $paths.ToArray()

    $script:CrateCommittedChangesCache[$cacheKey] = $result
    return $result
}

# For each published workspace crate, returns a hashtable folder -> ChangedFileCount
# where the count is the number of distinct repo-relative paths under crates/<folder>/
# that have changed since the crate's last release baseline (see
# Get-CrateLastReleaseBaseline). Considers:
#
#   - committed changes between the baseline and HEAD,
#   - tracked working-tree edits (staged + unstaged) vs HEAD,
#   - untracked files (e.g. new source files added during a release run).
#
# Crates with zero modifications are omitted from the result.
#
# Working-tree edits and untracked files are queried once globally and bucketed
# per crate to avoid spawning O(crates) extra git processes. The per-crate
# committed diff is served from Get-CrateCommittedChanges' session cache.
function Get-CratesWithUnreleasedChanges {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    $result = @{}
    $crates = Get-WorkspaceCrates -repoRoot $RepoRoot

    $workingByCrate = @{}
    $globalWorking   = Invoke-Git -Arguments @('diff', '--name-only', 'HEAD', '--') -RepoRoot $RepoRoot
    $globalUntracked = Invoke-Git -Arguments @('ls-files', '--others', '--exclude-standard') -RepoRoot $RepoRoot
    foreach ($line in @(@($globalWorking) + @($globalUntracked))) {
        $p = $line.ToString().Trim().Replace('\', '/')
        if ([string]::IsNullOrEmpty($p)) { continue }
        $folder = Get-CrateFolderForPath -Path $p
        if (-not $folder) { continue }
        if (-not $workingByCrate.ContainsKey($folder)) {
            $workingByCrate[$folder] = [System.Collections.Generic.HashSet[string]]::new()
        }
        [void]$workingByCrate[$folder].Add($p)
    }

    foreach ($crate in $crates) {
        if (-not $crate.Published) { continue }

        $folder = $crate.Folder
        $files = [System.Collections.Generic.HashSet[string]]::new()

        foreach ($p in Get-CrateCommittedChanges -RepoRoot $RepoRoot -CrateFolder $folder) {
            [void]$files.Add($p)
        }

        if ($workingByCrate.ContainsKey($folder)) {
            foreach ($p in $workingByCrate[$folder]) { [void]$files.Add($p) }
        }

        if ($files.Count -gt 0) {
            $result[$folder] = $files.Count
        }
    }

    return $result
}

# For every published workspace crate, compares the on-disk current version with the
# version at $BaseRef and returns the folders whose version differs. On-disk reads
# avoid cache staleness when this is called between mid-run Cargo.toml edits.
function Get-CratesWithVersionBumps {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef
    )

    $crates = Get-WorkspaceCrates -repoRoot $RepoRoot
    $bumped = [System.Collections.Generic.HashSet[string]]::new()

    foreach ($crate in $crates) {
        if (-not $crate.Published) { continue }

        $cargoToml = Join-Path $RepoRoot "crates/$($crate.Folder)/Cargo.toml"
        if (-not (Test-Path $cargoToml)) { continue }

        $currentVersion = Get-CurrentVersion -cargoTomlPath $cargoToml
        $baseVersion    = Get-CrateVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -CrateFolder $crate.Folder

        # New crate (not present at base) counts as "bumped" (definitely being released for the first time).
        if ($null -eq $baseVersion) {
            [void]$bumped.Add($crate.Folder)
            continue
        }

        if ($currentVersion -ne $baseVersion) {
            [void]$bumped.Add($crate.Folder)
        }
    }

    # PowerShell pipeline collapses an empty HashSet to $null on return; -NoEnumerate
    # preserves it so downstream .Contains() calls still work.
    Write-Output -NoEnumerate $bumped
}

# Returns a sorted array of pending-release records for every published workspace
# crate whose on-disk Cargo.toml version differs from the version at $BaseRef. Each
# record exposes the data the announcement formatter and base-relative re-invocation
# logic need:
#
#   [pscustomobject]@{
#     Folder         = '<crate folder under crates/>'
#     Name           = '<package name from Cargo.toml [package].name>'
#     BaseVersion    = '<version at BaseRef>'
#     CurrentVersion = '<version on disk>'
#   }
#
# New crates not present at $BaseRef are NOT included — they have no "base version"
# to compare against, and the rest of the script's flow treats them as fresh
# releases anyway (Invoke-CrateRelease writes the initial Cargo.toml + changelog
# entry). Only crates that genuinely have a prior committed version with a
# different on-disk version qualify as "pending" in the cross-invocation sense.
#
# Sorted ascending by Folder for deterministic output (the announcement order
# must be stable across runs / hosts / etc.).
function Get-PendingReleases {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$BaseRef
    )

    if ([string]::IsNullOrEmpty($BaseRef)) {
        return @()
    }

    $crates = Get-WorkspaceCrates -repoRoot $RepoRoot
    $pending = New-Object System.Collections.Generic.List[object]

    foreach ($crate in $crates) {
        if (-not $crate.Published) { continue }

        $cargoToml = Join-Path $RepoRoot "crates/$($crate.Folder)/Cargo.toml"
        if (-not (Test-Path $cargoToml)) { continue }

        $currentVersion = Get-CurrentVersion -cargoTomlPath $cargoToml
        $baseVersion    = Get-CrateVersionFromRef -RepoRoot $RepoRoot -BaseRef $BaseRef -CrateFolder $crate.Folder

        # New crate at base: skip (no base version to be pending against).
        if ($null -eq $baseVersion) { continue }
        if ($currentVersion -eq $baseVersion) { continue }

        $pending.Add([pscustomobject]@{
            Folder         = $crate.Folder
            Name           = $crate.Name
            BaseVersion    = $baseVersion
            CurrentVersion = $currentVersion
        }) | Out-Null
    }

    return @($pending | Sort-Object -Property Folder)
}

# --- CORE ANALYSIS ---
#
# For each crate in the "release set" (crates with version bumps vs base), walk its
# transitive normal/build workspace dependencies. Report any workspace dependency that
#
#   1. has source modifications since its own last release baseline (i.e. since the
#      most recent commit that touched its `version =` or `publish =` line — see
#      Get-CrateLastReleaseBaseline), and
#   2. is NOT itself in the release set, and
#   3. is published (publish != false),
#
# along with the shortest dependency chain that reaches it from a released crate.
#
# Per-crate baselines (rather than a global PR-vs-base-ref diff) are required to
# detect upstream changes that were merged to main in earlier PRs without a version
# bump and are now being depended on by a release-set crate in this PR. Comparing
# the working tree only against the PR base ref would miss those.
#
# Stops at any node already in the release set (its own bump pulls through changes).
#
# Returns @() when there are no findings, otherwise an array of objects:
#   Folder            - crate folder under crates/
#   PackageName       - cargo package name
#   CurrentVersion    - package's current version (Cargo.toml [package].version)
#   ChangedFileCount  - number of files changed under crates/<folder>/ since baseline
#   DependencyChains  - @( @('released_crate', 'mid_crate', 'this_dep'), ... )
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
        [Parameter(Mandatory = $true)][string]$BaseRef
    )

    $crates      = Get-WorkspaceCrates -repoRoot $RepoRoot
    $releaseSet  = Get-CratesWithVersionBumps -RepoRoot $RepoRoot -BaseRef $BaseRef
    $modifiedMap = Get-CratesWithUnreleasedChanges -RepoRoot $RepoRoot

    if ($releaseSet.Count -eq 0) { return @() }

    # Build folder -> crate lookup and normalized-name -> folder lookup.
    $byFolder = @{}
    $folderByNormName = @{}
    foreach ($c in $crates) {
        $byFolder[$c.Folder] = $c
        $folderByNormName[$c.Name.Replace('-', '_')] = $c.Folder
    }

    # Aggregate findings: folder -> { Folder; PackageName; ChangedFileCount; DependencyChains }.
    # Ordered so the BFS insertion order is preserved when iterating .Values; matters because
    # the post-release scan prompts the user in this order and a non-deterministic order
    # makes the UX flaky and tests unreliable.
    $findings = [ordered]@{}

    foreach ($releasedFolder in @($releaseSet | Sort-Object)) {
        if (-not $byFolder.ContainsKey($releasedFolder)) { continue }

        # BFS forward over normal+build deps. Track shortest path to each visited
        # node within this start-crate's traversal (avoids cycles and keeps the
        # recorded chain to the SHORTEST path from this entry point).
        $visited = [System.Collections.Generic.HashSet[string]]::new()
        [void]$visited.Add($releasedFolder)
        $queue = [System.Collections.Generic.Queue[object]]::new()
        $queue.Enqueue([pscustomobject]@{ Folder = $releasedFolder; Chain = @($releasedFolder) })

        while ($queue.Count -gt 0) {
            $node = $queue.Dequeue()
            $crate = $byFolder[$node.Folder]
            if ($null -eq $crate) { continue }

            foreach ($depNorm in $crate.Deps) {
                if (-not $folderByNormName.ContainsKey($depNorm)) { continue } # external crate
                $depFolder = $folderByNormName[$depNorm]
                if ($visited.Contains($depFolder)) { continue }
                [void]$visited.Add($depFolder)

                $depCrate = $byFolder[$depFolder]
                $depChain = $node.Chain + $depFolder

                # Only record this dep as a finding when it has unreleased
                # modifications, is published, AND is NOT already in the release
                # set (release-set members are being published as part of this
                # PR, so they are not findings themselves).
                if (-not $releaseSet.Contains($depFolder) -and `
                    $modifiedMap.ContainsKey($depFolder) -and $depCrate.Published) {
                    if (-not $findings.Contains($depFolder)) {
                        $findings[$depFolder] = [pscustomobject]@{
                            Folder           = $depFolder
                            PackageName      = $depCrate.Name
                            CurrentVersion   = $depCrate.Version
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

    if ($findings.Count -eq 0) { return @() }

    # Reduce each finding's chains: drop duplicates and shorter chains that are
    # strict suffixes of a longer chain, so the user sees only the longest
    # caller-rooted path through each branch.
    foreach ($f in $findings.Values) {
        $f.DependencyChains = Reduce-DependencyChains -Chains $f.DependencyChains
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
