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
      - Get-PackageLastReleaseBaseline -> the rev those paths were diffed
                                          against, recorded per compile-fixture
                                          obligation as baselineRev.

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

# A compile fixture is a consumer program whose *compile result* is the
# assertion: trybuild-style `tests/ui` and `tests/compile_fail` cases plus the
# sibling `.stderr`/`.stdout` files that record the expected failure. Their
# arrival, departure, or edit is the one mechanically visible trace a proc
# macro's compile contract leaves, and it frequently lands in the macro's
# runtime facade rather than in the macro crate itself -- which is exactly why
# it must be gathered across the whole macro review scope and not per package.
$script:CompileFixturePattern =
    '^crates/[^/]+/tests/(?:ui|compile_fail)/.+\.(?:rs|stderr|stdout)$'

function Get-CompileFixtureKind {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ($Path.EndsWith('.rs', [StringComparison]::Ordinal)) { return 'uiFixture' }
    return 'uiExpectation'
}

# Returns the sibling expectation paths for a fixture, or the fixture path for
# an expectation. Used to decide expectedResult and, in the resolver, to let one
# evidence entry discharge a fixture and its expectation files together.
function Get-CompileFixtureSiblings {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ($Path.EndsWith('.rs', [StringComparison]::Ordinal)) {
        $stem = $Path.Substring(0, $Path.Length - 3)
        return @("$stem.stderr", "$stem.stdout")
    }
    $stem = $Path.Substring(0, $Path.LastIndexOf('.'))
    return @("$stem.rs")
}

function Test-WorkingTreeFile {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$RelativePath
    )

    return Test-Path -LiteralPath (
        Join-Path $RepoRoot $RelativePath.Replace('/', '\')
    )
}

# Compile-fixture changes owned by one package, derived from the same diff that
# produced modifiedFiles. Status comes from presence at the baseline rev versus
# presence in the working tree, so a fixture added in an uncommitted edit and a
# fixture added in a commit are reported identically.
function Get-PackageCompileFixtureChanges {
    param(
        [Parameter(Mandatory = $true)][string]$PackageFolder,
        [Parameter(Mandatory = $true)][bool]$OwnerPublished,
        [AllowNull()]$ModifiedFiles
    )

    $candidates = @(
        @($ModifiedFiles) |
            Where-Object { $null -ne $_ } |
            ForEach-Object { $_.ToString().Replace('\', '/') } |
            Where-Object { $_ -match $script:CompileFixturePattern } |
            Sort-Object -Unique
    )
    if ($candidates.Count -eq 0) { return @() }

    $baselineRev = Get-PackageLastReleaseBaseline `
        -RepoRoot $RepoRoot `
        -PackageFolder $PackageFolder
    $baselineFiles = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    if (-not [string]::IsNullOrWhiteSpace($baselineRev)) {
        $tracked = Invoke-Git -Arguments @(
            'ls-tree',
            '-r',
            '--name-only',
            $baselineRev,
            '--',
            "crates/$PackageFolder/tests"
        ) -RepoRoot $RepoRoot -AllowFailure
        foreach ($line in @($tracked)) {
            $entry = $line.ToString().Trim().Replace('\', '/')
            if (-not [string]::IsNullOrWhiteSpace($entry)) {
                [void]$baselineFiles.Add($entry)
            }
        }
    }

    $items = New-Object 'System.Collections.Generic.List[object]'
    foreach ($path in $candidates) {
        $inBaseline = $baselineFiles.Contains($path)
        $inCurrent = Test-WorkingTreeFile -RepoRoot $RepoRoot -RelativePath $path
        $status = if (-not $inBaseline -and $inCurrent) {
            'added'
        } elseif ($inBaseline -and -not $inCurrent) {
            'removed'
        } elseif ($inBaseline -and $inCurrent) {
            'modified'
        } else {
            # Neither side has it: a transient path that came and went inside the
            # unreleased window. It asserts nothing about either revision.
            continue
        }

        $kind = Get-CompileFixtureKind -Path $path
        # A `.stderr`/`.stdout` file *is* a recorded compile failure; a `.rs`
        # case is only known to be compile-fail when such a sibling exists on
        # either side. Anything else stays null rather than guessing.
        $expectedResult = if ($kind -eq 'uiExpectation') {
            'fail'
        } else {
            $hasExpectation = $false
            foreach ($sibling in Get-CompileFixtureSiblings -Path $path) {
                if (
                    $baselineFiles.Contains($sibling) -or
                    (Test-WorkingTreeFile -RepoRoot $RepoRoot -RelativePath $sibling)
                ) {
                    $hasExpectation = $true
                    break
                }
            }
            if ($hasExpectation) { 'fail' } else { $null }
        }

        $items.Add([ordered]@{
                ownerPackage    = $PackageFolder
                ownerPublished  = $OwnerPublished
                path            = $path
                kind            = $kind
                status          = $status
                expectedResult  = $expectedResult
                baselineRev     = $baselineRev
            }) | Out-Null
    }

    # Hand back a materialised array: a List[object] is not safely wrapped by
    # @() on PowerShell 7.4. Dictionaries do not unroll in the pipeline, so the
    # caller's @(...) sees the items themselves.
    return $items.ToArray()
}

# True when the crate's own diff changes Rust implementation: any added or
# removed line in a packaged `.rs` file (src, build.rs, or a custom library
# path -- never under tests/, benches/, or examples/) that is not a doc
# comment, line comment, or blank. Doc-comment-only edits, README/CHANGELOG/
# Cargo.toml edits, and test/bench/example edits leave this false. A previously
# released library with no implementation change of its own therefore cannot be
# classified breaking or nonbreaking on its own account -- any elevation above
# patch must come from a cascade the resolver owns, not from a re-exported macro
# contract or a dependency bump the model read into the crate's own diff.
#
# Conservative by construction: a missing baseline or a brand-new untracked
# source file counts as an implementation change, so the guard downstream only
# ever fires on an unambiguous doc-only diff. Block comments (`/* ... */`) also
# read as implementation, keeping the fire condition to lines that unambiguously
# begin with `//`.
function Get-PackageRustImplementationChanged {
    param(
        [Parameter(Mandatory = $true)][string]$PackageFolder,
        [AllowNull()]$ModifiedFiles
    )

    $prefix = "crates/$PackageFolder/"
    $candidates = @(
        @($ModifiedFiles) |
            Where-Object { $null -ne $_ } |
            ForEach-Object { $_.ToString().Replace('\', '/') } |
            Where-Object {
                $_.EndsWith('.rs', [StringComparison]::OrdinalIgnoreCase) -and
                $_.StartsWith($prefix, [StringComparison]::Ordinal)
            } |
            Where-Object {
                $relative = $_.Substring($prefix.Length)
                -not (
                    $relative.StartsWith('tests/', [StringComparison]::Ordinal) -or
                    $relative.StartsWith('benches/', [StringComparison]::Ordinal) -or
                    $relative.StartsWith('examples/', [StringComparison]::Ordinal)
                )
            } |
            Sort-Object -Unique
    )
    if ($candidates.Count -eq 0) { return $false }

    $baselineRev = Get-PackageLastReleaseBaseline `
        -RepoRoot $RepoRoot `
        -PackageFolder $PackageFolder
    if ([string]::IsNullOrWhiteSpace($baselineRev)) { return $true }

    foreach ($path in $candidates) {
        $diff = @(
            Invoke-Git -Arguments @(
                'diff',
                '--no-ext-diff',
                '--no-color',
                '--unified=0',
                $baselineRev,
                '--',
                $path
            ) -RepoRoot $RepoRoot -AllowFailure
        )
        # A file listed as modified whose baseline-to-worktree diff is empty is an
        # untracked new source file (git diff cannot see it): an implementation
        # change by construction.
        if ($diff.Count -eq 0) { return $true }

        foreach ($line in $diff) {
            $text = $line.ToString()
            if ($text.Length -eq 0) { continue }
            $marker = $text[0]
            if ($marker -ne '+' -and $marker -ne '-') { continue }
            if ($text.StartsWith('+++') -or $text.StartsWith('---')) { continue }
            $content = $text.Substring(1).Trim()
            if ($content.Length -eq 0) { continue }
            if ($content.StartsWith('//')) { continue }
            return $true
        }
    }

    return $false
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

# An external dependency's requirement is part of the published manifest, so a
# consumer resolves against it directly. When the crate also names that
# dependency's types in its own public API, moving the requirement to another
# compatibility line hands consumers a different type identity under the same
# paths -- a break no cargo-semver-checks run on this workspace can see, because
# nothing in THIS crate's rustdoc changed.
#
# The current side comes from cargo metadata, which has already resolved
# [workspace.dependencies] inheritance. The baseline side has to be read out of
# Git text: cargo cannot be pointed at a historical revision without
# materialising a whole workspace checkout, and every package has its own
# baseline commit.
$script:BaselineWorkspaceRequirementsCache = @{}

function Get-BaselineWorkspaceRequirements {
    param([Parameter(Mandatory = $true)][string]$BaselineSha)

    if ($script:BaselineWorkspaceRequirementsCache.ContainsKey($BaselineSha)) {
        return $script:BaselineWorkspaceRequirementsCache[$BaselineSha]
    }

    $text = @(
        Invoke-Git -Arguments @('show', "${BaselineSha}:Cargo.toml") `
            -RepoRoot $RepoRoot -AllowFailure
    ) -join "`n"
    $requirements = Get-CargoWorkspaceRequirements -ManifestText $text
    $script:BaselineWorkspaceRequirementsCache[$BaselineSha] = $requirements
    return $requirements
}

function Get-ExternalDependencyChanges {
    param(
        [AllowNull()][string]$BaselineSha,
        [Parameter(Mandatory = $true)][string]$PackageFolder,
        [Parameter(Mandatory = $true)]$CurrentExternalDeps,
        [Parameter(Mandatory = $true)][System.Collections.Generic.HashSet[string]]$WorkspaceMemberNames
    )

    if ([string]::IsNullOrWhiteSpace($BaselineSha)) { return @() }

    $manifestText = @(
        Invoke-Git -Arguments @(
            'show',
            "${BaselineSha}:crates/$PackageFolder/Cargo.toml"
        ) -RepoRoot $RepoRoot -AllowFailure
    ) -join "`n"
    # No manifest at the baseline means the crate did not exist then; there is
    # no released requirement any change could invalidate.
    if ([string]::IsNullOrWhiteSpace($manifestText)) { return @() }

    $baselineDeps = Get-CargoManifestDependencies `
        -ManifestText $manifestText `
        -WorkspaceRequirements (Get-BaselineWorkspaceRequirements -BaselineSha $BaselineSha)

    $names = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($name in $baselineDeps.Keys) { [void]$names.Add($name) }
    foreach ($name in $CurrentExternalDeps.Keys) { [void]$names.Add($name) }

    $items = New-Object 'System.Collections.Generic.List[object]'
    foreach ($name in @($names | Sort-Object { $_ } -CaseSensitive)) {
        # A workspace member is released by this same plan, so its version
        # requirement is the cascade's business, not this lane's. Membership is
        # judged on the current workspace on both sides: a crate that moved
        # between the registry and the workspace changes identity for reasons
        # this comparison cannot express.
        if ($WorkspaceMemberNames.Contains($name)) { continue }

        $baselineRaw = $null
        if ($baselineDeps.Contains($name)) { $baselineRaw = $baselineDeps[$name].Requirement }
        $currentRaw = $null
        if ($CurrentExternalDeps.Contains($name)) { $currentRaw = $CurrentExternalDeps[$name].Requirement }

        $baselineReq = Get-NormalizedCargoRequirement -Requirement $baselineRaw
        $currentReq = Get-NormalizedCargoRequirement -Requirement $currentRaw
        if ($null -eq $baselineReq -and $null -eq $currentReq) { continue }
        if ($baselineReq -ceq $currentReq) { continue }

        $kinds = if ($CurrentExternalDeps.Contains($name)) {
            @($CurrentExternalDeps[$name].Kinds)
        } elseif ($baselineDeps.Contains($name)) {
            @($baselineDeps[$name].Kinds)
        } else {
            @()
        }

        $items.Add([ordered]@{
                name        = $name
                baselineReq = $baselineReq
                currentReq  = $currentReq
                kinds       = @($kinds)
                breaking    = [bool](Test-CargoRequirementBreaking `
                        -BaselineRequirement $baselineReq `
                        -CurrentRequirement $currentReq)
                baselineRev = $BaselineSha
            }) | Out-Null
    }

    return $items.ToArray()
}

$workspaceMemberNames = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
foreach ($package in $packages) {
    [void]$workspaceMemberNames.Add($package.Name.Replace('-', '_'))
}

$factPackages = foreach ($package in $packages) {
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

    $externalDepChanges = @(
        Get-ExternalDependencyChanges `
            -BaselineSha $baselineSha `
            -PackageFolder $package.Folder `
            -CurrentExternalDeps $package.ExternalDeps `
            -WorkspaceMemberNames $workspaceMemberNames
    )
    # Proc macros export behavior, not foreign type identity: a macro's public
    # surface is the syntax it accepts and the code it generates, and nothing a
    # consumer writes can name `syn::Error` through it. Its own dependency bumps
    # are therefore private by construction, and the compile-fixture lane is what
    # governs its contract.
    $externalExposedDeps = if ([bool]$package.IsProcMacroOnly) {
        @()
    } elseif ($uncheckedTarget) {
        @($package.ExternalDeps.Keys)
    } else {
        @(
            foreach ($externalName in $package.ExternalDeps.Keys) {
                $exposed = Test-PackageExposesTarget `
                    -Dependent $package `
                    -TargetPackageName $externalName
                if ($exposed) { $externalName }
            }
        )
    }

    # A requirement inherited from [workspace.dependencies] changes the crate's
    # PUBLISHED manifest -- cargo publish inlines the resolved value -- while
    # leaving every file under crates/<folder>/ untouched. Without this
    # promotion such a crate looks unmodified and never enters review at all.
    $externalScopes = @(
        @(
            foreach ($change in $externalDepChanges) { @($change.kinds) }
        ) | Where-Object { $_ } | Sort-Object -Unique
    )
    $hasExternalDepChange = $externalDepChanges.Count -gt 0

    # Whether the crate's own packaged Rust source (not tests/benches/examples,
    # not docs or manifests) actually changed. A previously released library with
    # only doc-comment, test, or manifest edits has no own-diff basis for a
    # breaking or nonbreaking classification -- the resolver still cascades one in
    # when a dependency or re-exported macro contract requires it.
    $rustImplementationChanged = if (
        $workspaceModifiedFiles.ContainsKey($package.Folder)
    ) {
        Get-PackageRustImplementationChanged `
            -PackageFolder $package.Folder `
            -ModifiedFiles $workspaceModifiedFiles[$package.Folder]
    } else {
        $false
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
            ($workspaceModifiedFiles.ContainsKey($package.Folder) -or $hasExternalDepChange)
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
        manifestDependencyScopes = @(
            # Keeps Get-ManifestChanges' scope order, then appends whatever the
            # external-dependency lane adds, so the existing sequence is stable.
            $(
                $seenScopes = [System.Collections.Generic.HashSet[string]]::new(
                    [System.StringComparer]::Ordinal
                )
                foreach ($scope in @($manifestChanges.DependencyScopes)) {
                    if ($seenScopes.Add($scope)) { $scope }
                }
                foreach ($scope in $externalScopes) {
                    if ($seenScopes.Add($scope)) { $scope }
                }
            )
        )
        manifestOtherChanged = [bool]$manifestChanges.OtherChanged
        # True only when packaged Rust source changed beyond doc comments; gates
        # an own-diff breaking/nonbreaking classification (see resolve-plan.ps1).
        rustImplementationChanged = [bool]$rustImplementationChanged
        workspaceModified = $workspaceModifiedFiles.ContainsKey($package.Folder) -or
            $hasExternalDepChange
        # Effective non-dev external dependency requirement changes between this
        # crate's release baseline and the working tree, inheritance resolved.
        externalDepChanges = @($externalDepChanges)
        # The subset of current external dependencies whose types this crate's
        # public API may name. Fail-closed: absent or unreadable exposure
        # metadata counts as exposed.
        externalExposedDeps = @($externalExposedDeps | Sort-Object -Unique)
        # Filled in by the macro review-scope pass below, once macroRuntimePartners
        # is complete. Always present so the resolver can validate the schema
        # uniformly; only proc-macro packages ever carry entries.
        macroCompileFixtureChanges = @()
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

# Compile-fixture obligations are gathered per proc macro across the SAME review
# scope the resolver already enforces (the macro itself, its modified
# implementation closure, and its modified runtime partners). Running after the
# runtime-partner back-fill is what lets a fixture added in the facade crate --
# where it is otherwise indistinguishable from an ordinary test-only edit --
# reach the macro whose compile contract it actually documents.
#
# scopeRole records WHY a fixture is in scope, because the two roles answer
# different questions. A fixture owned by the macro or by a facade that
# re-exports it is a consumer program for that macro, so its outcome speaks for
# the macro's compile contract. A fixture owned by a published implementation
# dependency is a consumer program for THAT crate, which carries its own release
# classification; it is still reported so the review cannot miss it, but the
# resolver does not let it set the macro's verdict floor.
$fixtureChangesByFolder = @{}
foreach ($fact in $factPackages) {
    $fixtureChangesByFolder[$fact.folder] = @(
        Get-PackageCompileFixtureChanges `
            -PackageFolder $fact.folder `
            -OwnerPublished ([bool]$fact.published) `
            -ModifiedFiles $fact.modifiedFiles
    )
}
foreach ($fact in $factPackages) {
    if (-not [bool]$fact.procMacroOnly) { continue }

    $partnerNames = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($partner in @($fact.macroRuntimePartners)) {
        [void]$partnerNames.Add($partner)
    }
    $closureNames = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($member in @($fact.macroImplementationClosure)) {
        [void]$closureNames.Add($member)
    }

    $scopeRoleByFolder = [ordered]@{}
    $scopeRoleByFolder[$fact.folder] = 'self'
    foreach ($candidate in $factPackages) {
        if ($candidate.folder -eq $fact.folder) { continue }
        if (-not [bool]$candidate.workspaceModified) { continue }
        $normalizedName = $candidate.name.Replace('-', '_')
        if ($partnerNames.Contains($normalizedName)) {
            $scopeRoleByFolder[$candidate.folder] = 'runtimePartner'
        } elseif ($closureNames.Contains($normalizedName)) {
            $scopeRoleByFolder[$candidate.folder] = 'implementationClosure'
        }
    }

    $byKey = @{}
    foreach ($folder in $scopeRoleByFolder.Keys) {
        foreach ($item in @($fixtureChangesByFolder[$folder])) {
            $scoped = [ordered]@{}
            foreach ($property in $item.Keys) { $scoped[$property] = $item[$property] }
            $scoped['scopeRole'] = $scopeRoleByFolder[$folder]
            $byKey["$($item.ownerPackage)`u{0000}$($item.path)"] = $scoped
        }
    }
    $orderedKeys = [string[]]@($byKey.Keys)
    [Array]::Sort($orderedKeys, [StringComparer]::Ordinal)
    $fact['macroCompileFixtureChanges'] = @(
        foreach ($key in $orderedKeys) { $byKey[$key] }
    )
}

[ordered]@{
    schemaVersion = 5
    repoRoot = $RepoRoot
    baseRef  = $BaseRef
    packages = @($factPackages)
} | ConvertTo-Json -Depth 8
