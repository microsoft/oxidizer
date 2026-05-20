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
      - Detecting which crates have been modified vs a base ref, which have had
        version bumps, and which workspace dependencies of in-release crates are
        modified-but-unreleased (the core "unreleased upstream dependency" analysis).

    It has no top-level param() block and no side effects beyond declaring script-scope
    caches & compiled regexes.
#>

# --- COMPILED REGEX PATTERNS ---

$script:ConventionalCommitRegex = [regex]'^(\w+)(?:\(.*\))?(!)?:\s*(.*)'
$script:PrReferenceRegex = [regex]'\s*(\(#(\d+)\))$'
$script:SemanticVersionRegex = [regex]'^\d+\.\d+\.\d+$'
$script:CargoVersionRegex = [regex]'(?<=version\s*=\s*")[^"]+'
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

    $v1Parts = $version1.Split('.') | ForEach-Object { [int]$_ }
    $v2Parts = $version2.Split('.') | ForEach-Object { [int]$_ }

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

function Test-IsBreakingChange {
    param(
        [string]$oldVersion,
        [ValidateSet('major', 'minor', 'patch')]
        [string]$bump
    )

    $parts = $oldVersion.Split('.') | ForEach-Object { [int]$_ }
    while ($parts.Count -lt 3) { $parts += 0 }

    if ($parts[0] -ge 1) {
        return $bump -eq 'major'
    }
    if ($parts[1] -ge 1) {
        return $bump -eq 'major'
    }
    return $true
}

# Reads the `version = "..."` from a Cargo.toml on disk.
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

# Reads the `version = "..."` from a crate's Cargo.toml as it exists at $BaseRef.
# Returns $null if the file does not exist at that ref (e.g. crate added in this PR).
function Get-CrateVersionFromRef {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef,
        [Parameter(Mandatory = $true)][string]$CrateFolder
    )

    $output = Invoke-Git -Arguments @('show', "${BaseRef}:crates/$CrateFolder/Cargo.toml") -RepoRoot $RepoRoot -AllowFailure
    if ($null -eq $output) { return $null }

    $content = ($output -join "`n")
    $m = $script:CargoVersionRegex.Match($content)
    if (-not $m.Success) { return $null }
    return $m.Value
}

# --- WORKSPACE METADATA ---

# Cached `cargo metadata --no-deps` for the workspace. Graph topology is safe to cache
# across nested release runs; mutable version data is read fresh from disk via
# Get-CurrentVersion to avoid staleness.
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

# Invalidates the cached metadata. Call this after editing any Cargo.toml in the
# workspace so subsequent analyses see fresh deps/versions.
function Invalidate-WorkspaceMetadataCache {
    $script:CachedWorkspaceMetadata = $null
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

# Repo-relative paths changed in the working tree vs the merge-base with $BaseRef.
# Considers both committed changes (merge-base..HEAD) and uncommitted working-tree edits
# (HEAD..working tree, untracked included). This makes the analysis correct for both the
# CI flow (everything committed) and the interactive flow (mid-script Cargo.toml edits
# that are not yet committed).
function Get-GitFileChangeSet {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef
    )

    $set = [System.Collections.Generic.HashSet[string]]::new()

    $mergeBaseOut = Invoke-Git -Arguments @('merge-base', $BaseRef, 'HEAD') -RepoRoot $RepoRoot
    $mergeBase    = (@($mergeBaseOut))[0]
    if ([string]::IsNullOrWhiteSpace($mergeBase)) {
        Write-Warning "Unable to determine merge-base with '$BaseRef'; falling back to '$BaseRef' as diff base."
        $mergeBase = $BaseRef
    }

    $committed = Invoke-Git -Arguments @('diff', '--name-only', $mergeBase, 'HEAD', '--') -RepoRoot $RepoRoot
    foreach ($line in $committed) {
        $trimmed = $line.ToString().Trim()
        if (-not [string]::IsNullOrEmpty($trimmed)) {
            [void]$set.Add($trimmed.Replace('\', '/'))
        }
    }

    # Uncommitted edits (tracked, staged + unstaged).
    $working = Invoke-Git -Arguments @('diff', '--name-only', 'HEAD', '--') -RepoRoot $RepoRoot
    foreach ($line in $working) {
        $trimmed = $line.ToString().Trim()
        if (-not [string]::IsNullOrEmpty($trimmed)) {
            [void]$set.Add($trimmed.Replace('\', '/'))
        }
    }

    # Untracked files (e.g. new source files added during a release).
    $untracked = Invoke-Git -Arguments @('ls-files', '--others', '--exclude-standard') -RepoRoot $RepoRoot
    foreach ($line in $untracked) {
        $trimmed = $line.ToString().Trim()
        if (-not [string]::IsNullOrEmpty($trimmed)) {
            [void]$set.Add($trimmed.Replace('\', '/'))
        }
    }

    # Preserve HashSet semantics across the function boundary. Without -NoEnumerate
    # an empty HashSet collapses to $null, which breaks downstream .Contains() calls.
    Write-Output -NoEnumerate $set
}

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

# Returns a deduped HashSet[string] of crate folder names that have at least one
# changed file under crates/<folder>/ vs the base ref.
function Get-CratesWithFileChanges {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef
    )

    $changes = Get-GitFileChangeSet -RepoRoot $RepoRoot -BaseRef $BaseRef
    $folders = [System.Collections.Generic.HashSet[string]]::new()
    foreach ($p in $changes) {
        $folder = Get-CrateFolderForPath -Path $p
        if ($folder) { [void]$folders.Add($folder) }
    }
    # See Get-GitFileChangeSet for why -NoEnumerate matters.
    Write-Output -NoEnumerate $folders
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

    # See Get-GitFileChangeSet for why -NoEnumerate matters.
    Write-Output -NoEnumerate $bumped
}

# --- CORE ANALYSIS ---
#
# For each crate in the "release set" (crates with version bumps vs base), walk its
# transitive normal/build workspace dependencies. Report any workspace dependency that
#
#   1. has file changes vs the base ref, AND
#   2. is NOT itself in the release set,
#
# along with the shortest dependency chain that reaches it from a released crate.
#
# Stops at any node already in the release set (its own bump pulls through changes).
#
# Returns @() when there are no findings, otherwise an array of objects:
#   Folder            - crate folder under crates/
#   PackageName       - cargo package name
#   ChangedFileCount  - number of files changed under crates/<folder>/
#   DependencyChains  - @( @('released_crate', 'mid_crate', 'this_dep'), ... )
function Get-UnreleasedModifiedDependencies {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseRef
    )

    $crates       = Get-WorkspaceCrates -repoRoot $RepoRoot
    $releaseSet   = Get-CratesWithVersionBumps -RepoRoot $RepoRoot -BaseRef $BaseRef
    $modified     = Get-CratesWithFileChanges -RepoRoot $RepoRoot -BaseRef $BaseRef
    $changedFiles = Get-GitFileChangeSet -RepoRoot $RepoRoot -BaseRef $BaseRef

    if ($releaseSet.Count -eq 0) { return @() }

    # Build folder -> crate lookup and normalized-name -> folder lookup.
    $byFolder = @{}
    $folderByNormName = @{}
    foreach ($c in $crates) {
        $byFolder[$c.Folder] = $c
        $folderByNormName[$c.Name.Replace('-', '_')] = $c.Folder
    }

    # Aggregate findings: folder -> { Folder; PackageName; ChangedFileCount; DependencyChains }
    $findings = @{}

    foreach ($releasedFolder in @($releaseSet)) {
        if (-not $byFolder.ContainsKey($releasedFolder)) { continue }

        # BFS forward over normal+build deps. Track shortest path to each visited node.
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

                if ($releaseSet.Contains($depFolder)) {
                    # Stop traversal — this dep's own bump pulls through downstream changes.
                    continue
                }

                if ($modified.Contains($depFolder) -and $depCrate.Published) {
                    if (-not $findings.ContainsKey($depFolder)) {
                        $changeCount = 0
                        $prefix = "crates/$depFolder/"
                        foreach ($p in $changedFiles) {
                            if ($p.StartsWith($prefix)) { $changeCount++ }
                        }
                        $findings[$depFolder] = [pscustomobject]@{
                            Folder           = $depFolder
                            PackageName      = $depCrate.Name
                            ChangedFileCount = $changeCount
                            DependencyChains = @(, $depChain)
                        }
                    }
                    else {
                        $existing = $findings[$depFolder]
                        $existing.DependencyChains = @($existing.DependencyChains) + @(, $depChain)
                    }
                }

                # Continue BFS past unchanged-and-unreleased intermediates so that a
                # hidden modified upstream dep (separated from the released crate by an
                # unchanged intermediate) is still detected.
                $queue.Enqueue([pscustomobject]@{ Folder = $depFolder; Chain = $depChain })
            }
        }
    }

    if ($findings.Count -eq 0) { return @() }
    return @($findings.Values)
}
