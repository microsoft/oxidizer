# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Emits the deterministic release "facts" for the workspace as JSON.

.DESCRIPTION
    This is a small, deterministic helper for the AI release skill
    (.github/skills/release-packages/SKILL.md). It does NOT make any release
    decisions and NEVER writes to the repository. It only gathers the objective
    facts an agent needs to plan a release, so that different reasoning models
    start from an identical, machine-checked fact base rather than re-deriving it
    (and possibly diverging) by hand-parsing `cargo metadata` and `git` output.

    The facts are read from the existing, tested release library
    (scripts/lib/releasing.ps1) so this script stays a thin shell:

      - Get-WorkspacePackages          -> folder / name / version / published /
                                          proc-macro-only / library-target /
                                          dependency and exposure edges
                                          (normal + build deps, dev excluded,
                                           names normalised with '-' -> '_').
      - Get-PreviousVersionBumpCommit  -> baseline commit sha for
                                          cargo-semver-checks (--baseline-rev).
      - Get-PackageUnreleasedChangeFiles -> exact paths changed under
                                           crates/<folder>/.

    Version-bump arithmetic, cascade resolution, change-type classification, and
    all file writes are intentionally NOT done here -- those belong to the skill
    (judgment + planning) and to cargo-semver-checks / release-changelog.ps1.

.PARAMETER RepoRoot
    Workspace root (directory containing the top-level Cargo.toml). Defaults to
    the repository this script lives in.

.PARAMETER BaseRef
    Git ref used as the "previous release" boundary for baseline-commit lookup.
    Defaults to HEAD (the last committed version bump == the previous release).

.EXAMPLE
    ./.github/skills/release-packages/scripts/release-facts.ps1 |
        ConvertFrom-Json
#>
[CmdletBinding()]
param(
    [string]$RepoRoot,
    [string]$BaseRef = 'HEAD'
)

$ErrorActionPreference = 'Stop'

$skillRepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
$libraryPath = Join-Path $skillRepoRoot 'scripts\lib\releasing.ps1'
if (-not (Test-Path -LiteralPath $libraryPath)) {
    throw "The release skill requires the shared library at '$libraryPath'."
}
. $libraryPath

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = $skillRepoRoot
} else {
    $RepoRoot = (Resolve-Path $RepoRoot).Path
}

# Start from a clean cache so repeated invocations (e.g. between mid-plan edits)
# never read stale cargo-metadata or git results.
Reset-ReleaseScriptCaches

# Validate BaseRef once, up front, for a single clear failure. An unresolvable or
# un-fetched ref cannot yield meaningful facts. Get-PreviousVersionBumpCommit runs
# the same Test-GitRef internally and already throws on a bad ref, so this is not
# guarding against silent swallowing -- it just fails once here rather than once
# per package.
if (-not (Test-GitRef -Ref $BaseRef -RepoRoot $RepoRoot)) {
    throw "Base ref '$BaseRef' could not be resolved in '$RepoRoot'. Ensure it is fetched (CI should checkout with fetch-depth: 0) and spelled correctly."
}

$packages = @(Get-WorkspacePackages -repoRoot $RepoRoot)
$workspaceModifiedFiles = Get-PackageUnreleasedChangeFiles `
    -RepoRoot $RepoRoot `
    -IncludeUnpublished

$packageByName = @{}
foreach ($package in $packages) {
    $packageByName[$package.Name.Replace('-', '_')] = $package
}

function Get-ReachableWorkspacePackages {
    param([Parameter(Mandatory = $true)][pscustomobject]$Package)

    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $queue = [System.Collections.Generic.Queue[string]]::new()
    foreach ($dependency in @($Package.Deps)) {
        $queue.Enqueue($dependency)
    }

    while ($queue.Count -gt 0) {
        $dependency = $queue.Dequeue()
        if (-not $packageByName.ContainsKey($dependency)) { continue }
        if (-not $seen.Add($dependency)) { continue }
        foreach ($transitiveDependency in @($packageByName[$dependency].Deps)) {
            $queue.Enqueue($transitiveDependency)
        }
    }

    return @($seen | Sort-Object)
}

function Get-ManifestChanges {
    param(
        [Parameter(Mandatory = $true)][string]$BaselineSha,
        [Parameter(Mandatory = $true)][string]$PackageFolder
    )

    $manifestPath = "crates/$PackageFolder/Cargo.toml"
    $diff = @(
        Invoke-Git -Arguments @(
            'diff',
            '--no-ext-diff',
            '--no-color',
            '--unified=999999',
            $BaselineSha,
            '--',
            $manifestPath
        ) -RepoRoot $RepoRoot
    )
    if ($diff.Count -eq 0) {
        return [pscustomobject]@{
            DependencyScopes = @()
            OtherChanged = $false
        }
    }

    function New-OrdinalCountMap {
        return [System.Collections.Generic.Dictionary[string, int]]::new(
            [System.StringComparer]::Ordinal
        )
    }

    $changes = @{ old = @{}; new = @{} }
    foreach ($side in @('old', 'new')) {
        foreach ($scope in @('normal', 'build', 'dev', 'features', 'metadata', 'other')) {
            $changes[$side][$scope] = New-OrdinalCountMap
        }
    }
    $oldSection = ''
    $newSection = ''

    function Get-DependencyScope {
        param([string]$Section)

        $normalizedSection = $Section.ToLowerInvariant()
        if (
            $normalizedSection -match '^package\.metadata(?:\.|$)' -or
            $normalizedSection -match '^lints(?:\.|$)'
        ) {
            return 'metadata'
        }
        $match = [regex]::Match(
            $normalizedSection,
            '^(?:target\..+\.)?(dependencies|build-dependencies|dev-dependencies)(?:\.|$)|^(features)$'
        )
        if (-not $match.Success) { return $null }
        $kind = if ($match.Groups[1].Success) {
            $match.Groups[1].Value
        } else {
            $match.Groups[2].Value
        }
        $scope = switch ($kind) {
            'dependencies'       { 'normal' }
            'build-dependencies' { 'build' }
            'dev-dependencies'   { 'dev' }
            'features'           { 'features' }
        }
        return $scope
    }

    function Add-ScopedChange {
        param(
            [Parameter(Mandatory = $true)][string]$Side,
            [AllowEmptyString()][string]$Section,
            [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Content
        )

        $inSingle = $false
        $inDouble = $false
        $escaped = $false
        $commentIndex = -1
        for ($i = 0; $i -lt $Content.Length; $i++) {
            $character = $Content[$i]
            if ($inDouble -and $escaped) {
                $escaped = $false
                continue
            }
            if ($inDouble -and $character -eq '\') {
                $escaped = $true
                continue
            }
            if (-not $inDouble -and $character -eq "'") {
                $inSingle = -not $inSingle
                continue
            }
            if (-not $inSingle -and $character -eq '"') {
                $inDouble = -not $inDouble
                continue
            }
            if (-not $inSingle -and -not $inDouble -and $character -eq '#') {
                $commentIndex = $i
                break
            }
        }
        $normalized = if ($commentIndex -ge 0) {
            $Content.Substring(0, $commentIndex).Trim()
        } else {
            $Content.Trim()
        }
        if (
            [string]::IsNullOrWhiteSpace($normalized)
        ) {
            return
        }
        $scope = Get-DependencyScope -Section $Section
        if ([string]::IsNullOrWhiteSpace($scope)) { $scope = 'other' }
        $key = "$Section`0$normalized"
        $map = $changes[$Side][$scope]
        $count = if ($map.ContainsKey($key)) { $map[$key] } else { 0 }
        $map[$key] = $count + 1
    }

    foreach ($record in $diff) {
        if ($record -isnot [string]) { continue }
        $line = [string]$record
        if (
            $line.StartsWith('diff --git ', [StringComparison]::Ordinal) -or
            $line.StartsWith('index ', [StringComparison]::Ordinal) -or
            $line.StartsWith('--- ', [StringComparison]::Ordinal) -or
            $line.StartsWith('+++ ', [StringComparison]::Ordinal) -or
            $line.StartsWith('@@ ', [StringComparison]::Ordinal)
        ) {
            continue
        }
        if ($line.Length -eq 0 -or $line[0] -notin @(' ', '+', '-')) {
            continue
        }

        $content = $line.Substring(1)
        $sectionMatch = [regex]::Match(
            $content,
            '^\s*(?:\[\[([^\]]+)\]\]|\[([^\]]+)\])\s*(?:#.*)?$'
        )
        $section = if ($sectionMatch.Groups[1].Success) {
            $sectionMatch.Groups[1].Value.Trim()
        } elseif ($sectionMatch.Groups[2].Success) {
            $sectionMatch.Groups[2].Value.Trim()
        } else {
            $null
        }
        switch ($line[0]) {
            ' ' {
                if ($null -ne $section) {
                    $oldSection = $section
                    $newSection = $oldSection
                } else {
                    if ($oldSection -cne $newSection) {
                        Add-ScopedChange -Side old -Section $oldSection -Content $content
                        Add-ScopedChange -Side new -Section $newSection -Content $content
                    }
                }
            }
            '-' {
                if ($null -ne $section) {
                    $oldSection = $section
                } else {
                    Add-ScopedChange `
                        -Side old `
                        -Section $oldSection `
                        -Content $content
                }
            }
            '+' {
                if ($null -ne $section) {
                    $newSection = $section
                } else {
                    Add-ScopedChange `
                        -Side new `
                        -Section $newSection `
                        -Content $content
                }
            }
        }
    }

    $changedScopes = @(
        foreach ($scope in @('normal', 'build', 'dev', 'features', 'metadata', 'other')) {
            $keys = [System.Collections.Generic.HashSet[string]]::new(
                [System.StringComparer]::Ordinal
            )
            foreach ($key in $changes.old[$scope].Keys) { [void]$keys.Add($key) }
            foreach ($key in $changes.new[$scope].Keys) { [void]$keys.Add($key) }
            $changed = $false
            foreach ($key in @($keys | Sort-Object)) {
                $oldCount = if ($changes.old[$scope].ContainsKey($key)) {
                    $changes.old[$scope][$key]
                } else {
                    0
                }
                $newCount = if ($changes.new[$scope].ContainsKey($key)) {
                    $changes.new[$scope][$key]
                } else {
                    0
                }
                if (
                    $oldCount -ne $newCount
                ) {
                    $changed = $true
                    break
                }
            }
            if ($changed) { $scope }
        }
    )
    return [pscustomobject]@{
        DependencyScopes = @(
            $changedScopes |
                Where-Object { $_ -notin @('metadata', 'other') }
        )
        OtherChanged = $changedScopes -contains 'other'
    }
}

$factPackages = foreach ($package in $packages) {
    $baselineSha = $null
    if (
        @($package.MacroRuntimePartners).Count -gt 0 -and
        -not [bool]$package.IsProcMacroOnly
    ) {
        throw "Package '$($package.Folder)' declares macro_runtime but is not proc-macro-only."
    }
    foreach ($partner in @($package.MacroRuntimePartners)) {
        if (-not $packageByName.ContainsKey($partner)) {
            throw "Package '$($package.Folder)' declares unknown macro_runtime partner '$partner'."
        }
        if (-not [bool]$packageByName[$partner].Published) {
            throw "Package '$($package.Folder)' declares unpublished macro_runtime partner '$partner'."
        }
    }
    $uncheckedTarget = -not [bool]$package.IsProcMacroOnly -and -not [bool]$package.HasLibraryTarget
    $reachablePackages = @(Get-ReachableWorkspacePackages -Package $package)
    $exposedPackages = if ([bool]$package.IsProcMacroOnly) {
        @()
    } elseif ($uncheckedTarget) {
        @($package.Deps)
    } else {
        @(
            foreach ($targetName in $reachablePackages) {
                $target = $packageByName[$targetName]
                if ($null -eq $target) { continue }
                if ([bool]$target.IsProcMacroOnly) { continue }

                $exposed = if (@($package.Deps) -contains $targetName) {
                    Test-PackageExposesTarget `
                        -Dependent $package `
                        -TargetPackageName $target.Name
                } else {
                    Test-PackageAllowlistNamesTarget `
                        -Dependent $package `
                        -TargetPackageName $target.Name `
                        -TargetCrateRoot $target.CrateRoot
                }
                if ($exposed) { $targetName }
            }
        )
    }
    $macroPublicPackages = @(
        foreach ($targetName in $reachablePackages) {
            $target = $packageByName[$targetName]
            if ($null -eq $target -or -not [bool]$target.IsProcMacroOnly) {
                continue
            }

            $isDirect = @($package.Deps) -contains $targetName
            $published = if ($isDirect) {
                Test-PackageAllowlistNamesDirectTarget `
                    -Dependent $package `
                    -TargetPackageName $target.Name
            } else {
                Test-PackageAllowlistNamesTarget `
                    -Dependent $package `
                    -TargetPackageName $target.Name `
                    -TargetCrateRoot $target.CrateRoot `
                    -WildcardIsEvidence $false
            }
            if ($published) { $targetName }
        }
    )

    # baselineSha is the crate's previous version-bump commit, or null if none can
    # be found. Note a crate's introducing commit counts as a bump, so in practice
    # even a never-released crate gets a baselineSha (its own first commit) -- use
    # the everReleased fact below, NOT hasBaseline, to tell a first-ever release
    # apart from a real one.
    $bump = Get-PreviousVersionBumpCommit -RepoRoot $RepoRoot -BaseRef $BaseRef -PackageFolder $package.Folder
    if ($null -ne $bump) { $baselineSha = $bump.Sha }
    $manifestChanges = if ($null -ne $baselineSha) {
        Get-ManifestChanges `
            -BaselineSha $baselineSha `
            -PackageFolder $package.Folder
    } else {
        [pscustomobject]@{
            DependencyScopes = @()
            OtherChanged = $false
        }
    }

    [ordered]@{
        folder            = $package.Folder
        name              = $package.Name
        version           = $package.Version
        published         = [bool]$package.Published
        procMacroOnly     = [bool]$package.IsProcMacroOnly
        hasLibraryTarget  = [bool]$package.HasLibraryTarget
        deps              = @($package.Deps)
        # Includes direct exposure edges and positively identified indirect
        # re-export edges to transitively reachable workspace packages.
        exposedDeps       = @($exposedPackages | Sort-Object -Unique)
        # Proc-macro entry points are behavioral contracts, not Rust type
        # identities. Keep their public re-export edges separate so a macro's
        # reviewed contract change, rather than its Cargo version, controls
        # breaking propagation.
        macroPublicDeps   = @($macroPublicPackages | Sort-Object -Unique)
        macroImplementationClosure = if ([bool]$package.IsProcMacroOnly) {
            @($reachablePackages)
        } else {
            @()
        }
        macroRuntimePartners = @($package.MacroRuntimePartners)
        exposureUnknown   = $uncheckedTarget
        baselineSha       = $baselineSha
        hasBaseline       = ($null -ne $baselineSha)
        # Whether the crate has ever been published, determined from its release
        # tags. A crate's introducing commit counts as a version bump, so
        # hasBaseline alone cannot distinguish a first-ever release from a real one;
        # cargo-semver-checks against an unpublished baseline would classify normal
        # pre-publication churn as breaking. The release skill's Step 3 branches on
        # this fact.
        everReleased      = [bool](Invoke-Git -Arguments @('tag', '--list', "$($package.Name)-v*") -RepoRoot $RepoRoot)
        modified          = [bool]$package.Published -and
            $workspaceModifiedFiles.ContainsKey($package.Folder)
        modifiedFiles     = if ($workspaceModifiedFiles.ContainsKey($package.Folder)) {
            @($workspaceModifiedFiles[$package.Folder])
        } else {
            @()
        }
        modifiedFileCount = if ($workspaceModifiedFiles.ContainsKey($package.Folder)) {
            @($workspaceModifiedFiles[$package.Folder]).Count
        } else {
            0
        }
        manifestDependencyScopes = @($manifestChanges.DependencyScopes)
        manifestOtherChanged = [bool]$manifestChanges.OtherChanged
        workspaceModified = $workspaceModifiedFiles.ContainsKey($package.Folder)
    }
}

$factByName = @{}
foreach ($fact in $factPackages) {
    $factByName[$fact.name.Replace('-', '_')] = $fact
}
foreach ($dependent in $factPackages) {
    if (-not [bool]$dependent.published) { continue }
    foreach ($macroName in @($dependent.macroPublicDeps)) {
        $macroFact = $factByName[$macroName]
        if ($null -eq $macroFact) { continue }
        $macroFact['macroRuntimePartners'] = @(
            @($macroFact.macroRuntimePartners) +
                $dependent.name.Replace('-', '_') |
                Sort-Object -Unique
        )
    }
}

[ordered]@{
    schemaVersion = 4
    repoRoot = $RepoRoot
    baseRef  = $BaseRef
    packages = @($factPackages)
} | ConvertTo-Json -Depth 6
