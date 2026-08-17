# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Resolves a deterministic release plan from facts and model classifications.

.DESCRIPTION
    Performs only mechanical work: token parsing, version arithmetic, dependency
    closure, type-exposure and macro-contract propagation, pin reconciliation,
    and topological ordering. The release skill remains responsible for
    classifying source diffs and reviewing proc-macro behavior.

.PARAMETER FactsPath
    JSON emitted by release-facts.ps1.

.PARAMETER RequestPath
    JSON with mode, tokens, selectionDecisions, classifications,
    macroContracts, and optional force:
      {
        "mode": "targeted",
        "tokens": ["bytesbuf@breaking"],
        "selectionDecisions": {},
        "classifications": {
          "bytesbuf": "patch",
          "bytesbuf_io": { "changeType": "patch", "manualReview": false }
        },
        "macroContracts": {
          "templated_uri_macros": {
            "verdict": "compatible",
            "reviewedPackages": [
              "templated_uri_macros",
              "templated_uri_macros_impl"
            ],
            "channels": {
              "exportedMacros": "unchanged",
              "acceptedSyntax": "unchanged",
              "compileBehavior": "unchanged",
              "generatedApi": "unchanged",
              "generatedRuntimePaths": "unchanged",
              "hygiene": "unchanged"
            },
            "evidence": ["Expansion snapshots and compile fixtures are unchanged."],
            "compileEvidence": [
              {
                "ownerPackage": "templated_uri",
                "path": "crates/templated_uri/tests/ui/bad_template.rs",
                "baseline": { "revision": "<sha>", "result": "fail", "exitCode": 101 },
                "current":  { "revision": "worktree", "result": "fail", "exitCode": 101 }
              }
            ]
          }
        },
        "force": false
      }

    compileEvidence is required for every fixture the facts report changed in the
    macro's review scope (macroCompileFixtureChanges). Measured outcomes derive a
    verdict floor -- pass to fail is breaking, fail to pass is nonbreaking, an
    unchanged outcome is compatible -- and a declared verdict below that floor
    blocks the plan.

    regressionEvidence is required for every selection decision whose reason is
    behavior-fix (changed/all mode):
      "selectionDecisions": {
        "cachet_tier": {
          "decision": "accept",
          "reason": "behavior-fix",
          "evidence": ["Eviction now honors the configured tier bound."],
          "regressionEvidence": [
            {
              "kind": "consumer-runtime",
              "probe": "cargo test -p cachet_tier --test eviction",
              "baseline": { "revision": "<sha>", "result": "fail", "exitCode": 101 },
              "current":  { "revision": "worktree", "result": "pass", "exitCode": 0 }
            }
          ]
        }
      }

    Each entry pairs one consumer-runtime, consumer-compile, or packaged-artifact
    probe measured at the release baseline and at the current revision; only a
    baseline failure that now passes demonstrates the fix. Any other outcome, or
    a measurement that cannot be read, blocks the plan.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$FactsPath,

    [Parameter(Mandatory = $true)]
    [string]$RequestPath
)

$ErrorActionPreference = 'Stop'

$skillRepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
$libraryPath = Join-Path $skillRepoRoot 'scripts\lib\releasing.ps1'
if (-not (Test-Path -LiteralPath $libraryPath)) {
    throw "The release skill requires the shared library at '$libraryPath'."
}
. $libraryPath

function ConvertTo-InternalChangeType {
    param(
        [AllowNull()][string]$Value,
        [switch]$AllowNone
    )

    if ([string]::IsNullOrWhiteSpace($Value)) {
        if ($AllowNone) { return 'none' }
        throw 'A change type is required.'
    }

    switch ($Value.ToLowerInvariant()) {
        'breaking'     { return 'breaking' }
        'nonbreaking'  { return 'non-breaking' }
        'non-breaking' { return 'non-breaking' }
        'patch'        { return 'patch' }
        'none'         {
            if ($AllowNone) { return 'none' }
            throw "Change type 'none' is not valid here."
        }
        default { throw "Unknown change type '$Value'." }
    }
}

function Get-StrongerChangeType {
    param(
        [Parameter(Mandatory = $true)][string]$Left,
        [Parameter(Mandatory = $true)][string]$Right
    )

    if ($script:ChangeTypeRank[$Left] -ge $script:ChangeTypeRank[$Right]) {
        return $Left
    }
    return $Right
}

function Get-RequestValue {
        param(
            [AllowNull()]$Container,
            [Parameter(Mandatory = $true)]$Fact
        )

        if ($null -eq $Container) { return $null }
        $property = $Container.PSObject.Properties[$Fact.folder]
        if ($null -eq $property) {
            $property = $Container.PSObject.Properties[$Fact.name]
        }
        if ($null -eq $property) { return $null }
        return $property.Value
}

# The measured half of a before/after probe: which revision was exercised, what
# it did, and the exit status that proves it. Compile fixtures and behaviour
# probes share this parser so they cannot drift apart on what counts as a usable
# measurement.
function Get-MeasuredOutcome {
    param([AllowNull()]$Value)

    if ($null -eq $Value -or $Value -is [string]) {
        return [pscustomobject]@{
            Result = $null; Revision = $null; ExitCode = $null; Complete = $false
        }
    }

    $result = ($Value.result ?? '').ToString().Trim().ToLowerInvariant()
    $revision = ($Value.revision ?? '').ToString().Trim()
    $exitCodeValue = $Value.exitCode
    $exitCode = $null
    if ($null -ne $exitCodeValue -and $exitCodeValue -is [ValueType]) {
        $exitCode = [int]$exitCodeValue
    } elseif (
        $exitCodeValue -is [string] -and
        [int]::TryParse($exitCodeValue, [ref]$null)
    ) {
        $exitCode = [int]$exitCodeValue
    }

    $complete = (
        $result -in @('pass', 'fail') -and
        -not [string]::IsNullOrWhiteSpace($revision) -and
        $null -ne $exitCode
    )
    return [pscustomobject]@{
        Result   = if ($complete) { $result } else { $null }
        Revision = $revision
        ExitCode = $exitCode
        Complete = $complete
    }
}

$script:RegressionEvidenceKinds = @(
    'consumer-runtime',
    'consumer-compile',
    'packaged-artifact'
)

# A behaviour fix is a claim about observable behaviour, so it is only credible
# when the same probe is shown failing at the release baseline and passing now.
# Each entry pairs the two runs of one probe, which is what makes "the same
# probe" mechanically checkable rather than a narrative assertion.
function Get-RegressionEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$Package,
        [AllowNull()]$Value
    )

    $entries = New-Object 'System.Collections.Generic.List[object]'
    $issues = New-Object 'System.Collections.Generic.List[string]'
    $demonstrated = $false

    foreach ($item in @($Value)) {
        if ($null -eq $item) { continue }
        if ($item -is [string]) {
            throw "Regression evidence in selection decision '$Package' must be an object with kind, probe, baseline, and current."
        }
        $probe = ($item.probe ?? '').ToString().Trim()
        if ([string]::IsNullOrWhiteSpace($probe)) {
            throw "Regression evidence in selection decision '$Package' must name the probe it exercised."
        }
        $kind = ($item.kind ?? '').ToString().Trim().ToLowerInvariant()
        if ($kind -notin $script:RegressionEvidenceKinds) {
            throw "Regression evidence '$probe' in selection decision '$Package' must use kind $($script:RegressionEvidenceKinds -join ', ')."
        }

        $baseline = Get-MeasuredOutcome -Value $item.baseline
        $current = Get-MeasuredOutcome -Value $item.current
        foreach ($side in @(
                [pscustomobject]@{ Name = 'baseline'; Outcome = $baseline },
                [pscustomobject]@{ Name = 'current'; Outcome = $current }
            )) {
            if (-not $side.Outcome.Complete) {
                $issues.Add("Regression evidence '$probe' in selection decision '$Package' does not record a $($side.Name) pass/fail result with a revision and exit code.") |
                    Out-Null
                continue
            }
            # An exit code that contradicts the recorded result means the
            # measurement was mis-transcribed; neither half can be trusted.
            if (($side.Outcome.Result -eq 'pass') -ne ($side.Outcome.ExitCode -eq 0)) {
                $issues.Add("Regression evidence '$probe' in selection decision '$Package' records a $($side.Name) result of '$($side.Outcome.Result)' with exit code $($side.Outcome.ExitCode).") |
                    Out-Null
            }
        }

        $outcome = $null
        if (
            $baseline.Complete -and $current.Complete -and
            ($baseline.Result -eq 'pass') -eq ($baseline.ExitCode -eq 0) -and
            ($current.Result -eq 'pass') -eq ($current.ExitCode -eq 0)
        ) {
            if ($baseline.Revision -eq $current.Revision) {
                # One revision measured twice compares nothing.
                $issues.Add("Regression evidence '$probe' in selection decision '$Package' measures revision '$($baseline.Revision)' on both sides.") |
                    Out-Null
            } else {
                $outcome = "$($baseline.Result)->$($current.Result)"
                if ($baseline.Result -eq 'fail' -and $current.Result -eq 'pass') {
                    $demonstrated = $true
                }
            }
        }

        $entries.Add([ordered]@{
                kind    = $kind
                probe   = $probe
                outcome = $outcome ?? 'inconclusive'
            }) | Out-Null
    }

    $ordered = $entries.ToArray() |
        Sort-Object -Property @{ Expression = { "$($_.kind)`u{0000}$($_.probe)" } }
    return [pscustomobject]@{
        Entries      = @($ordered)
        Issues       = @($issues | Sort-Object -Unique)
        Demonstrated = $demonstrated
    }
}

function Get-SelectionDecision {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [Parameter(Mandatory = $true)]$Request,
        [Parameter(Mandatory = $true)][string]$Mode
    )

    $value = $Request.selectionDecisions.PSObject.Properties[$Fact.folder].Value
    if ($null -eq $value -or $value -is [string]) {
        throw "Selection decision '$($Fact.folder)' must include decision, reason, and evidence."
    }

    $decision = ($value.decision ?? '').ToString().ToLowerInvariant()
    if ($decision -notin @('accept', 'decline')) {
        throw "Selection decision '$($Fact.folder)' must be accept or decline."
    }

    $reason = ($value.reason ?? '').ToString().ToLowerInvariant()
    $acceptedReasons = @(
        'breaking',
        'nonbreaking-api',
        'behavior-fix',
        'authored-doc-fix',
        'runtime-manifest-change',
        'first-release',
        'explicit-release'
    )
    $declinedReasons = @(
        'test-only',
        'benchmark-only',
        'dev-dependency-only',
        'release-metadata-only',
        'generated-artifact-only',
        'internal-only',
        'unchanged'
    )
    $allowedReasons = if ($decision -eq 'accept') {
        $acceptedReasons
    } else {
        $declinedReasons
    }
    if ($reason -notin $allowedReasons) {
        throw "Selection decision '$($Fact.folder)' has invalid $decision reason '$reason'."
    }
    if (
        $reason -eq 'explicit-release' -and
        ($Mode -ne 'all' -or [bool]$Fact.modified)
    ) {
        throw "Selection reason 'explicit-release' is only valid for an unchanged package in all mode."
    }
    $manifestDependencyScopes = @($Fact.manifestDependencyScopes)
    $hasRuntimeDependencyChange = (
        $manifestDependencyScopes -contains 'normal' -or
        $manifestDependencyScopes -contains 'build' -or
        $manifestDependencyScopes -contains 'features'
    )
    if ($reason -eq 'runtime-manifest-change' -and -not $hasRuntimeDependencyChange) {
        throw "Selection reason 'runtime-manifest-change' for '$($Fact.folder)' requires a changed normal/build dependency or package feature declaration."
    }
    if ($decision -eq 'decline' -and $hasRuntimeDependencyChange) {
        throw "Selection decision '$($Fact.folder)' cannot decline a changed normal/build dependency or package feature declaration."
    }
    # A published manifest dependency change is more consequential than a doc
    # tweak, so it owns the reason: `authored-doc-fix` cannot be paired with a
    # normal/build/features change. (A real API addition still elevates to
    # nonbreaking-api, and an exposed breaking dependency to breaking.)
    if ($reason -eq 'authored-doc-fix' -and $hasRuntimeDependencyChange) {
        throw "Selection reason 'authored-doc-fix' for '$($Fact.folder)' cannot be used alongside a normal/build dependency or package feature change; use 'runtime-manifest-change'."
    }
    $packagePrefix = "crates/$($Fact.folder)/"
    $otherFiles = @(
        $Fact.modifiedFiles |
            Where-Object { $null -ne $_ } |
            ForEach-Object { $_.ToString().Replace('\', '/') } |
            Where-Object {
                if (-not $_.StartsWith($packagePrefix, [StringComparison]::Ordinal)) {
                    return $true
                }
                $relative = $_.Substring($packagePrefix.Length)
                return $relative -notin @('Cargo.toml', 'README.md', 'CHANGELOG.md')
            }
    )
    if (
        $decision -eq 'accept' -and
        $manifestDependencyScopes.Count -eq 1 -and
        $manifestDependencyScopes[0] -eq 'dev' -and
        -not [bool]$Fact.manifestOtherChanged -and
        $otherFiles.Count -eq 0
    ) {
        throw "Selection decision '$($Fact.folder)' cannot accept a dev-dependency-only manifest change."
    }
    if (
        $decision -eq 'decline' -and
        $manifestDependencyScopes.Count -eq 1 -and
        $manifestDependencyScopes[0] -eq 'dev' -and
        -not [bool]$Fact.manifestOtherChanged -and
        $otherFiles.Count -eq 0 -and
        $reason -ne 'dev-dependency-only'
    ) {
        throw "Selection decision '$($Fact.folder)' must classify a pure dev dependency manifest edit as 'dev-dependency-only'."
    }
    if ($reason -eq 'dev-dependency-only') {
        if (
            $manifestDependencyScopes -notcontains 'dev' -or
            $hasRuntimeDependencyChange -or
            [bool]$Fact.manifestOtherChanged
        ) {
            throw "Selection reason 'dev-dependency-only' for '$($Fact.folder)' requires only changed dev dependency declarations and ignorable release metadata."
        }
        if ($otherFiles.Count -gt 0) {
            throw "Selection reason 'dev-dependency-only' for '$($Fact.folder)' cannot ignore changed source, tests, benchmarks, or authored documentation."
        }
    }
    # A generated artifact is exactly this crate's own README or CHANGELOG.
    # "Generated-only" means at least one file changed and every changed file is
    # one of those: it forces `generated-artifact-only` and forbids it once any
    # Cargo.toml (metadata) or other path is present, so the two decline reasons
    # are mutually exclusive and each diff shape has one canonical reason.
    $changedFiles = @(
        $Fact.modifiedFiles |
            Where-Object { $null -ne $_ } |
            ForEach-Object { $_.ToString().Replace('\', '/') }
    )
    $isGeneratedOnly = $changedFiles.Count -gt 0 -and @(
        $changedFiles | Where-Object {
            -not (
                $_.StartsWith($packagePrefix, [StringComparison]::Ordinal) -and
                $_.Substring($packagePrefix.Length) -in @('README.md', 'CHANGELOG.md')
            )
        }
    ).Count -eq 0
    if ($reason -eq 'generated-artifact-only' -and -not $isGeneratedOnly) {
        throw "Selection reason 'generated-artifact-only' for '$($Fact.folder)' requires that only this crate's generated README.md or CHANGELOG.md changed; a Cargo.toml or other edit is 'release-metadata-only'."
    }
    if ($reason -eq 'release-metadata-only' -and $isGeneratedOnly) {
        throw "Selection reason 'release-metadata-only' for '$($Fact.folder)' cannot classify a change to only a generated README.md or CHANGELOG.md; use 'generated-artifact-only'."
    }
    # A rustdoc-visible doc comment changed (facts field docCommentChanged) while
    # rustImplementationChanged is false: the crate's own diff is documentation
    # only, which ships in rustdoc and is consumer-visible. With no
    # runtime-manifest change and no exposed breaking external dependency to
    # elevate it, the one canonical outcome is accept `authored-doc-fix` -- it is
    # not an `internal-only` refactor (the docs did change) nor any other
    # decline. A plain `//` comment or whitespace edit leaves docCommentChanged
    # false and stays eligible for `internal-only`. Proc macros are governed by
    # their contract, not this rule.
    if (
        [bool]$Fact.everReleased -and
        -not [bool]$Fact.procMacroOnly -and
        [bool]$Fact.docCommentChanged -and
        -not [bool]$Fact.rustImplementationChanged -and
        -not $hasRuntimeDependencyChange -and
        @(
            @($Fact.externalDepChanges) |
                Where-Object {
                    [bool]$_.breaking -and @($Fact.externalExposedDeps) -contains $_.name
                }
        ).Count -eq 0 -and
        -not ($decision -eq 'accept' -and $reason -eq 'authored-doc-fix')
    ) {
        throw "Selection decision '$($Fact.folder)' changes a rustdoc-visible doc comment with no implementation change; a consumer-visible doc change must be accepted as 'authored-doc-fix', not '$reason'."
    }
    if ($decision -eq 'accept' -and -not [bool]$Fact.everReleased) {
        if ($reason -ne 'first-release') {
            throw "Never-released package '$($Fact.folder)' must use selection reason 'first-release'."
        }
        $packagePrefix = "crates/$($Fact.folder)/"
        $releaseWorthyFiles = @(
            $Fact.modifiedFiles |
                Where-Object { $null -ne $_ } |
                ForEach-Object { $_.ToString().Replace('\', '/') } |
                Where-Object {
                    if (-not $_.StartsWith($packagePrefix, [StringComparison]::Ordinal)) {
                        return $false
                    }
                    $relative = $_.Substring($packagePrefix.Length)
                    return (
                        $relative.StartsWith('src/', [StringComparison]::Ordinal) -or
                        $relative.StartsWith('examples/', [StringComparison]::Ordinal) -or
                        (
                            $relative.StartsWith('docs/', [StringComparison]::Ordinal) -and
                            $relative.EndsWith('.md', [StringComparison]::OrdinalIgnoreCase)
                        ) -or
                        $relative -eq 'build.rs'
                    )
                }
        )
        if ($releaseWorthyFiles.Count -eq 0) {
            throw "Selection reason 'first-release' for '$($Fact.folder)' requires a changed packaged file outside tests, benchmarks, and generated artifacts."
        }
    }

    $evidence = @(
        $value.evidence |
            Where-Object { $null -ne $_ } |
            ForEach-Object { $_.ToString().Trim() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if ($evidence.Count -eq 0) {
        throw "Selection decision '$($Fact.folder)' must include evidence."
    }

    $regression = Get-RegressionEvidence `
        -Package $Fact.folder `
        -Value $value.regressionEvidence

    return [pscustomobject]@{
        Fact               = $Fact
        Decision           = $decision
        Reason             = $reason
        Evidence           = $evidence
        RegressionEvidence = $regression.Entries
        EvidenceIssues     = $regression.Issues
        RegressionShown    = $regression.Demonstrated
    }
}

function Get-CompileFixtureKey {
    param([Parameter(Mandatory = $true)][string]$Path)

    # A `.stderr`/`.stdout` file is the recorded outcome of its `.rs` sibling, so
    # both collapse onto the fixture that was actually compiled. One measurement
    # of that fixture therefore discharges the whole group.
    $normalized = $Path.ToString().Trim().Replace('\', '/')
    foreach ($extension in @('.stderr', '.stdout')) {
        if ($normalized.EndsWith($extension, [StringComparison]::OrdinalIgnoreCase)) {
            return $normalized.Substring(0, $normalized.Length - $extension.Length) + '.rs'
        }
    }
    return $normalized
}

function ConvertTo-MacroVerdictName {
    param([Parameter(Mandatory = $true)][string]$ChangeType)

    switch ($ChangeType) {
        'breaking'     { return 'breaking' }
        'non-breaking' { return 'nonbreaking' }
        default        { return 'compatible' }
    }
}

function Get-CompileEvidenceOutcome {
    param(
        [Parameter(Mandatory = $true)][string]$Baseline,
        [Parameter(Mandatory = $true)][string]$Current
    )

    # The only mechanical reading of a compile fixture: what the same consumer
    # program did before the change versus after it.
    if ($Baseline -eq 'pass' -and $Current -eq 'fail') { return 'breaking' }
    if ($Baseline -eq 'fail' -and $Current -eq 'pass') { return 'non-breaking' }
    return 'patch'
}

function ConvertTo-CompileEvidenceSide {
    param(
        [Parameter(Mandatory = $true)][string]$Package,
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Side,
        [AllowNull()]$Value
    )

    $issue = "Compile evidence for '$Path' in macro contract '$Package' does not record a $Side pass/fail result with a revision and exit code."
    $outcome = Get-MeasuredOutcome -Value $Value
    if (-not $outcome.Complete) {
        return [pscustomobject]@{
            Result   = $null
            Revision = $outcome.Revision
            ExitCode = $outcome.ExitCode
            Issue    = $issue
        }
    }

    return [pscustomobject]@{
        Result   = $outcome.Result
        Revision = $outcome.Revision
        ExitCode = $outcome.ExitCode
        Issue    = $null
    }
}

function Get-MacroCompileEvidence {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [AllowNull()]$Value
    )

    $entries = New-Object 'System.Collections.Generic.List[object]'
    $issues = New-Object 'System.Collections.Generic.List[string]'
    $floor = 'patch'
    $deciding = New-Object 'System.Collections.Generic.List[string]'

    # A fixture owned by a published implementation dependency is a consumer
    # program for that crate, not for this macro: that crate carries its own
    # release classification, and letting its fixture set this macro's floor
    # would break every macro that merely depends on it. Everything else -- the
    # macro itself, the facades that re-export it, and unpublished helpers with
    # no release identity of their own -- can only be speaking about this macro.
    $nonFloorKeys = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($obligation in @($Fact.macroCompileFixtureChanges)) {
        if (
            ($obligation.scopeRole ?? '').ToString() -eq 'implementationClosure' -and
            [bool]$obligation.ownerPublished
        ) {
            $owner = ($obligation.ownerPackage ?? '').ToString().Replace('-', '_')
            [void]$nonFloorKeys.Add(
                "$owner|$(Get-CompileFixtureKey -Path ($obligation.path ?? ''))"
            )
        }
    }

    foreach ($item in @($Value)) {
        if ($null -eq $item) { continue }
        if ($item -is [string]) {
            throw "Compile evidence in macro contract '$($Fact.folder)' must be an object with ownerPackage, path, baseline, and current."
        }
        $ownerPackage = ($item.ownerPackage ?? '').ToString().Trim()
        $path = ($item.path ?? '').ToString().Trim()
        if (
            [string]::IsNullOrWhiteSpace($ownerPackage) -or
            [string]::IsNullOrWhiteSpace($path)
        ) {
            throw "Compile evidence in macro contract '$($Fact.folder)' must name ownerPackage and path."
        }

        $baseline = ConvertTo-CompileEvidenceSide `
            -Package $Fact.folder -Path $path -Side 'baseline' -Value $item.baseline
        $current = ConvertTo-CompileEvidenceSide `
            -Package $Fact.folder -Path $path -Side 'current' -Value $item.current
        foreach ($side in @($baseline, $current)) {
            if ($null -ne $side.Issue) { $issues.Add($side.Issue) | Out-Null }
        }

        $outcome = $null
        if ($null -ne $baseline.Result -and $null -ne $current.Result) {
            $outcome = Get-CompileEvidenceOutcome `
                -Baseline $baseline.Result `
                -Current $current.Result
            $evidenceKey = "$($ownerPackage.Replace('-', '_'))|$(Get-CompileFixtureKey -Path $path)"
            if (-not $nonFloorKeys.Contains($evidenceKey)) {
                $stronger = Get-StrongerChangeType -Left $floor -Right $outcome
                if ($stronger -ne $floor) {
                    $floor = $stronger
                    $deciding.Clear()
                }
                if ($outcome -eq $floor -and $outcome -ne 'patch') {
                    $deciding.Add($path) | Out-Null
                }
            }
        }

        $entries.Add([pscustomobject]@{
                OwnerPackage = $ownerPackage.Replace('-', '_')
                Path         = $path.Replace('\', '/')
                Key          = Get-CompileFixtureKey -Path $path
                Baseline     = $baseline
                Current      = $current
                Outcome      = $outcome
            }) | Out-Null
    }

    return [pscustomobject]@{
        Entries      = $entries.ToArray()
        Issues       = @($issues | Sort-Object -Unique)
        DerivedFloor = $floor
        Deciding     = @($deciding | Sort-Object -Unique)
    }
}

function Get-MacroContract {
        param(
            [Parameter(Mandatory = $true)]$Fact,
            [Parameter(Mandatory = $true)]$Request
        )

        if (-not [bool]$Fact.procMacroOnly) { return $null }
        $value = Get-RequestValue -Container $Request.macroContracts -Fact $Fact
        if ($null -eq $value) { return $null }

        $verdict = if ($value -is [string]) { $value } else { $value.verdict }
        $changeType = switch (($verdict ?? '').ToString().ToLowerInvariant()) {
            'compatible'  { 'patch' }
            'nonbreaking' { 'non-breaking' }
            'non-breaking' { 'non-breaking' }
            'breaking'    { 'breaking' }
            default {
                throw "Unknown macro-contract verdict '$verdict' for '$($Fact.folder)'."
            }
        }

        if ($value -is [string]) {
            throw "Macro contract '$($Fact.folder)' must include reviewedPackages, channels, and evidence."
        }
        foreach ($requiredProperty in @('reviewedPackages', 'channels', 'evidence')) {
            if (
                $null -eq $value.PSObject.Properties[$requiredProperty] -or
                $null -eq $value.$requiredProperty
            ) {
                throw "Macro contract '$($Fact.folder)' must include reviewedPackages, channels, and evidence."
            }
        }
        $reviewedPackages = @(
            $value.reviewedPackages |
                Where-Object { $null -ne $_ } |
                ForEach-Object { $_.ToString().Trim() } |
                Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
        )
        if ($reviewedPackages.Count -eq 0) {
            throw "Macro contract '$($Fact.folder)' must include at least one reviewed package."
        }

        $requiredChannels = @(
            'exportedMacros',
            'acceptedSyntax',
            'compileBehavior',
            'generatedApi',
            'generatedRuntimePaths',
            'hygiene'
        )
        foreach ($channel in $requiredChannels) {
            $property = $value.channels.PSObject.Properties[$channel]
            if ($null -eq $property -or
                $null -eq $property.Value -or
                $property.Value.ToString().ToLowerInvariant() -notin @('unchanged', 'changed', 'notapplicable')) {
                throw "Macro contract '$($Fact.folder)' must classify channel '$channel' as unchanged, changed, or notApplicable."
            }
        }

        $evidence = @(
            $value.evidence |
                Where-Object { $null -ne $_ } |
                ForEach-Object { $_.ToString().Trim() } |
                Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
        )
        if ($evidence.Count -eq 0) {
            throw "Macro contract '$($Fact.folder)' must include evidence."
        }

        $compileEvidence = Get-MacroCompileEvidence `
            -Fact $Fact `
            -Value $value.compileEvidence

        return [pscustomobject]@{
            Verdict         = $verdict.ToString().ToLowerInvariant().Replace('-', '')
            ChangeType      = $changeType
            ReviewedPackages = $reviewedPackages
            Channels        = $value.channels
            Evidence        = $evidence
            CompileEvidence = $compileEvidence.Entries
            EvidenceIssues  = $compileEvidence.Issues
            DerivedFloor    = $compileEvidence.DerivedFloor
            DecidingFixtures = $compileEvidence.Deciding
        }
}

function Get-MacroReviewScope {
        param(
            [Parameter(Mandatory = $true)]$Fact,
            [Parameter(Mandatory = $true)][object[]]$Facts,
            [AllowNull()]$TriggerFact
        )

        $scope = New-Object 'System.Collections.Generic.List[string]'
        $scope.Add($Fact.name) | Out-Null
        $closure = @($Fact.macroImplementationClosure)
        foreach ($candidate in $Facts) {
            $normalizedName = $candidate.name.Replace('-', '_')
            if (
                ($closure -contains $normalizedName -and [bool]$candidate.workspaceModified) -or
                (@($Fact.macroRuntimePartners) -contains $normalizedName -and [bool]$candidate.workspaceModified)
            ) {
                $scope.Add($candidate.name) | Out-Null
            }
        }
        if ($null -ne $TriggerFact) {
            $scope.Add($TriggerFact.name) | Out-Null
        }
        return @($scope | Sort-Object -Unique)
}

# The canonical review scope emitted in the plan: exactly self plus every
# modified implementation-closure member and modified runtime partner. The
# resolver validates that a supplied contract COVERS this scope, so a model may
# review more; emitting the computed scope rather than the model's list keeps the
# output identical regardless of any extra, unmodified packages a model chose to
# name.
function Get-EmittedReviewScope {
    param([Parameter(Mandatory = $true)][string]$Folder)

    $fact = @($facts | Where-Object { $_.folder -eq $Folder })[0]
    if ($null -eq $fact) { return @() }
    return @(Get-MacroReviewScope -Fact $fact -Facts $facts -TriggerFact $null)
}

function Test-MacroContractCoversScope {
        param(
            [Parameter(Mandatory = $true)]$Contract,
            [Parameter(Mandatory = $true)][string[]]$Scope
        )

        $reviewed = @(
            $Contract.ReviewedPackages |
                Where-Object { $null -ne $_ } |
                ForEach-Object { $_.ToString().Replace('-', '_') }
        )
        foreach ($identifier in $Scope) {
            if ($reviewed -notcontains $identifier.Replace('-', '_')) {
                return $false
            }
        }
        return $true
}

function Get-Classification {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [Parameter(Mandatory = $true)]$Request
    )

    $value = Get-RequestValue -Container $Request.classifications -Fact $Fact

    $manualReview = [bool]$Fact.procMacroOnly
    $changeType = $null
    if ($value -is [string]) {
        $changeType = ConvertTo-InternalChangeType -Value $value
    } elseif ($null -ne $value) {
        $changeType = ConvertTo-InternalChangeType -Value $value.changeType
        if (
            $null -ne $value.PSObject.Properties['manualReview'] -and
            [bool]$value.manualReview -ne $manualReview
        ) {
            throw "manualReview for '$($Fact.folder)' is resolver-owned and must be '$manualReview'."
        }
    }

    if (-not [bool]$Fact.everReleased) {
        return [pscustomobject]@{
            ChangeType   = 'none'
            ManualReview = $manualReview
        }
    }

    if ([string]::IsNullOrWhiteSpace($changeType)) {
        if ([bool]$Fact.procMacroOnly) {
            $changeType = 'patch'
            $manualReview = $true
        } else {
            throw "Missing objective classification for published package '$($Fact.folder)'."
        }
    }

    $macroContract = Get-MacroContract -Fact $Fact -Request $Request
    if ($null -ne $macroContract) {
        if ($null -ne $value -and $changeType -ne $macroContract.ChangeType) {
            throw "Classification '$changeType' for proc macro '$($Fact.folder)' conflicts with macro-contract verdict '$($macroContract.ChangeType)'."
        }
        $changeType = $macroContract.ChangeType
        $manualReview = $true
    }

    return [pscustomobject]@{
        ChangeType   = $changeType
        ManualReview = $manualReview
    }
}

function Find-PackageFact {
    param(
        [Parameter(Mandatory = $true)][string]$Identifier,
        [Parameter(Mandatory = $true)][object[]]$Facts
    )

    $normalized = $Identifier.Replace('-', '_')
    $matches = @(
        $Facts | Where-Object {
            $_.folder -eq $Identifier -or $_.name.Replace('-', '_') -eq $normalized
        }
    )
    if ($matches.Count -ne 1) {
        throw "Release token '$Identifier' matched $($matches.Count) workspace packages."
    }
    return $matches[0]
}

function Get-EntryTargetVersion {
    param([Parameter(Mandatory = $true)]$Entry)

    if (-not [string]::IsNullOrWhiteSpace($Entry.RequestedPin)) {
        return $Entry.RequestedPin
    }
    if (-not [bool]$Entry.Fact.everReleased) {
        return $Entry.Fact.version
    }
    return Get-NextVersion `
        -currentVersion $Entry.Fact.version `
        -ChangeType $Entry.EffectiveChangeType
}

function Assert-PinSatisfiesRequirement {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)][string]$RequiredChangeType,
        [Parameter(Mandatory = $true)][bool]$Force,
        [Parameter(Mandatory = $true)]$Warnings
    )

    if ([string]::IsNullOrWhiteSpace($Entry.RequestedPin) -or
        -not [bool]$Entry.Fact.everReleased) {
        return
    }

    $requiredTarget = Get-NextVersion `
        -currentVersion $Entry.Fact.version `
        -ChangeType $RequiredChangeType
    if ((Compare-SemanticVersions -version1 $Entry.RequestedPin -version2 $requiredTarget) -ge 0) {
        return
    }

    $message = "Explicit pin '$($Entry.RequestedPin)' for '$($Entry.Fact.folder)' is below the required '$requiredTarget' ($RequiredChangeType)."
    if (-not $Force) {
        throw $message
    }
    $Warnings.Add("$message Force keeps the pin while preserving the stronger change type for further cascade decisions.") | Out-Null
}

$factsDocument = Get-Content -LiteralPath (Resolve-Path $FactsPath) -Raw | ConvertFrom-Json
$request = Get-Content -LiteralPath (Resolve-Path $RequestPath) -Raw | ConvertFrom-Json
if ($factsDocument.schemaVersion -ne 5) {
    throw 'The facts document uses an unsupported schema. Rerun release-facts.ps1.'
}
$facts = @($factsDocument.packages)
if ($facts.Count -eq 0) {
    throw 'The facts document contains no workspace packages.'
}
foreach ($fact in $facts) {
    foreach ($requiredProperty in @(
            'macroPublicDeps',
            'macroImplementationClosure',
            'macroRuntimePartners',
            'macroCompileFixtureChanges',
            'externalDepChanges',
            'externalExposedDeps',
            'rustImplementationChanged',
            'docCommentChanged',
            'modifiedFiles',
            'manifestDependencyScopes',
            'manifestOtherChanged',
            'workspaceModified'
        )) {
        if ($null -eq $fact.PSObject.Properties[$requiredProperty]) {
            throw "Package fact '$($fact.folder)' is missing '$requiredProperty'. Rerun release-facts.ps1."
        }
    }
}

$mode = if ([string]::IsNullOrWhiteSpace($request.mode)) { 'targeted' } else { $request.mode.ToLowerInvariant() }
if ($mode -notin @('targeted', 'changed', 'all')) {
    throw "Unknown release mode '$mode'."
}

$force = [bool]$request.force
$selectionDecisions = @{}
if ($mode -in @('changed', 'all')) {
    $selectionCandidates = @(
        $facts |
            Where-Object {
                [bool]$_.published -and
                ($mode -eq 'all' -or [bool]$_.modified)
            }
    )
    if ($null -eq $request.selectionDecisions) {
        throw "Release mode '$mode' requires selectionDecisions."
    }
    $candidateFolders = @($selectionCandidates.folder | Sort-Object)
    $decisionKeys = @(
        $request.selectionDecisions.PSObject.Properties.Name |
            Sort-Object
    )
    $unknownKeys = @($decisionKeys | Where-Object { $_ -notin $candidateFolders })
    if ($unknownKeys.Count -gt 0) {
        throw "Selection decisions contain unknown or non-candidate packages: $($unknownKeys -join ', '). Use canonical folder identifiers."
    }
    $missingKeys = @($candidateFolders | Where-Object { $_ -notin $decisionKeys })
    if ($missingKeys.Count -gt 0) {
        throw "Selection decisions are missing candidate packages: $($missingKeys -join ', ')."
    }
    foreach ($fact in $selectionCandidates) {
        $selectionDecisions[$fact.folder] = Get-SelectionDecision `
            -Fact $fact `
            -Request $request `
            -Mode $mode
    }
}
$tokens = @($request.tokens)
if ($tokens.Count -eq 0 -and $mode -eq 'targeted') {
    throw "Release mode '$mode' requires at least one accepted package token."
}
$warnings = New-Object 'System.Collections.Generic.List[string]'
$ambiguities = New-Object 'System.Collections.Generic.List[object]'
$ambiguityKeys = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
$usedMacroContracts = @{}
$plan = @{}
$queue = New-Object 'System.Collections.Generic.Queue[string]'
$tokenFolders = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)

function Get-ModifiedMacroScopeMember {
    param([Parameter(Mandatory = $true)]$Fact)

    $scopeNames = @($Fact.macroImplementationClosure) +
        @($Fact.macroRuntimePartners)
    return @(
        $facts |
            Where-Object {
                [bool]$_.workspaceModified -and
                $scopeNames -contains $_.name.Replace('-', '_')
            }
    )
}

# The external dependency requirements a crate publishes are resolved by its
# consumers, so moving one to another compatibility line while the crate's
# public API names that dependency's types changes those types' identity for
# every consumer -- under unchanged paths, and invisibly to a
# cargo-semver-checks run that only sees this workspace's own rustdoc.
#
# The floor is deliberately narrow. A private dependency bump reaches no
# consumer, and a proc macro exports behaviour rather than foreign type
# identity, so neither can raise it; both are already excluded from
# externalExposedDeps by release-facts.ps1.
function Get-ExternalBreakingExposure {
    param([Parameter(Mandatory = $true)]$Fact)

    if (-not [bool]$Fact.everReleased) { return @() }

    $exposed = @($Fact.externalExposedDeps)
    return @(
        @($Fact.externalDepChanges) |
            Where-Object { [bool]$_.breaking -and $exposed -contains $_.name } |
            Sort-Object -Property name
    )
}

function Format-ExternalExposureProbe {
    param([Parameter(Mandatory = $true)]$Changes)

    return @(
        foreach ($change in @($Changes)) {
            [ordered]@{
                name        = $change.name
                baselineReq = $change.baselineReq
                currentReq  = $change.currentReq
            }
        }
    )
}

function Register-ExternalExposure {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$ChangeType
    )

    $flooring = @(Get-ExternalBreakingExposure -Fact $Fact)
    if ($flooring.Count -eq 0) { return }
    if (
        -not [string]::IsNullOrWhiteSpace($ChangeType) -and
        $script:ChangeTypeRank[$ChangeType] -ge $script:ChangeTypeRank['breaking']
    ) {
        return
    }

    $key = "$($Fact.folder)|externalExposureUnderclassified"
    if (-not $ambiguityKeys.Add($key)) { return }
    $ambiguities.Add([ordered]@{
            kind          = 'externalExposureUnderclassified'
            package       = $Fact.folder
            classified    = $ChangeType
            derivedFloor  = 'breaking'
            dependencies  = @(Format-ExternalExposureProbe -Changes $flooring)
            requiredInput = "classifications.$($Fact.folder)"
        }) | Out-Null
}

# A previously released ordinary library may only be classified breaking or
# nonbreaking on its own account when its own packaged Rust source actually
# changed. Doc comments, tests, benchmarks, examples, README/CHANGELOG, and
# manifest edits are not an own-diff basis for elevation above patch, and a
# re-exported macro contract or an exposed dependency bump is a cascade the
# resolver applies -- not something the crate declares about itself. The
# external-exposure lane already forces breaking up when a foreign type break is
# exposed, so it is exempt here; proc macros are classified by their contract,
# and first releases have no prior surface to break.
function Register-OwnDiffFloor {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$ChangeType
    )

    if (-not [bool]$Fact.everReleased) { return }
    if ([bool]$Fact.procMacroOnly) { return }
    if ([string]::IsNullOrWhiteSpace($ChangeType)) { return }
    if ($script:ChangeTypeRank[$ChangeType] -le $script:ChangeTypeRank['patch']) { return }
    if ([bool]$Fact.rustImplementationChanged) { return }
    if (@(Get-ExternalBreakingExposure -Fact $Fact).Count -gt 0) { return }

    $key = "$($Fact.folder)|ownClassificationUnsupported"
    if (-not $ambiguityKeys.Add($key)) { return }
    $ambiguities.Add([ordered]@{
            kind          = 'ownClassificationUnsupported'
            package       = $Fact.folder
            classified    = $ChangeType
            requiredInput = "classifications.$($Fact.folder)"
        }) | Out-Null
}

# A behaviour fix must be demonstrated, not asserted: some consumer-visible
# probe has to fail at the release baseline and pass at the current revision.
# Anything else -- no probe at all, an unchanged outcome, a newly broken probe,
# or a measurement that cannot be read -- blocks the plan instead of seeding a
# release, because an internal adaptation that preserves behaviour is
# indistinguishable from a fix once the reason is written down.
function Register-SelectionEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$Folder,
        [Parameter(Mandatory = $true)]$Decision
    )

    $exposureFlooring = @(Get-ExternalBreakingExposure -Fact $Decision.Fact)
    if ($exposureFlooring.Count -gt 0 -and $Decision.Reason -ne 'breaking') {
        # Declining, or accepting under any softer reason, records a judgement
        # the manifest contradicts. The selection reason has to agree with the
        # derived floor or the plan cannot be trusted to size the bump.
        $key = "$Folder|externalExposureUnderselected"
        if ($ambiguityKeys.Add($key)) {
            $ambiguities.Add([ordered]@{
                    kind          = 'externalExposureUnderselected'
                    package       = $Folder
                    decision      = $Decision.Decision
                    reason        = $Decision.Reason
                    derivedFloor  = 'breaking'
                    dependencies  = @(Format-ExternalExposureProbe -Changes $exposureFlooring)
                    requiredInput = "selectionDecisions.$Folder.reason"
                }) | Out-Null
        }
    }

    if ($Decision.Reason -eq 'breaking') {
        $classification = Get-Classification -Fact $Decision.Fact -Request $request
        if ($classification.ChangeType -ne 'breaking') {
            $key = "$Folder|breakingSelectionUnderclassified"
            if ($ambiguityKeys.Add($key)) {
                $ambiguities.Add([ordered]@{
                        kind                    = 'breakingSelectionUnderclassified'
                        package                 = $Folder
                        reason                  = $Decision.Reason
                        objectiveClassification = ConvertTo-MacroVerdictName `
                            -ChangeType $classification.ChangeType
                        requiredInput           = "selectionDecisions.$Folder.reason"
                    }) | Out-Null
            }
        }
    }

    if ($Decision.Reason -ne 'behavior-fix') { return }

    if (@($Decision.EvidenceIssues).Count -gt 0) {
        $inconclusiveKey = "$Folder|behaviorEvidenceInconclusive"
        if ($ambiguityKeys.Add($inconclusiveKey)) {
            $ambiguities.Add([ordered]@{
                    kind          = 'behaviorEvidenceInconclusive'
                    package       = $Folder
                    reason        = $Decision.Reason
                    issues        = @($Decision.EvidenceIssues)
                    requiredInput = "selectionDecisions.$Folder.regressionEvidence"
                }) | Out-Null
        }
    }

    if (-not $Decision.RegressionShown) {
        $undemonstratedKey = "$Folder|behaviorFixUndemonstrated"
        if ($ambiguityKeys.Add($undemonstratedKey)) {
            $ambiguities.Add([ordered]@{
                    kind          = 'behaviorFixUndemonstrated'
                    package       = $Folder
                    reason        = $Decision.Reason
                    probes        = @($Decision.RegressionEvidence)
                    requiredInput = "selectionDecisions.$Folder.regressionEvidence"
                }) | Out-Null
        }
    }
}

function Register-MacroContract {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [Parameter(Mandatory = $true)]$Contract,
        [Parameter(Mandatory = $true)][string]$Trigger
    )

    $blocked = $false

    # Every fixture the facts saw change in this macro's review scope is an
    # obligation: the contract must say what that consumer program did before
    # the change and what it does now. Without it a compile-contract break in a
    # fixture owned by a runtime partner is indistinguishable from a test-only
    # edit, which is precisely how a rejected input can ship as a patch.
    $obligations = @($Fact.macroCompileFixtureChanges)
    if ($obligations.Count -gt 0) {
        $evidenceKeys = [System.Collections.Generic.HashSet[string]]::new(
            [System.StringComparer]::Ordinal
        )
        foreach ($entry in @($Contract.CompileEvidence)) {
            [void]$evidenceKeys.Add("$($entry.OwnerPackage)|$($entry.Key)")
        }
        $missing = @(
            foreach ($obligation in $obligations) {
                $owner = ($obligation.ownerPackage ?? '').ToString().Replace('-', '_')
                $key = Get-CompileFixtureKey -Path ($obligation.path ?? '')
                if (-not $evidenceKeys.Contains("$owner|$key")) {
                    $obligation.path
                }
            }
        ) | Sort-Object -Unique
        if ($missing.Count -gt 0) {
            $missingKey = "$($Fact.folder)|macroCompileFixtureUnevidenced"
            if ($ambiguityKeys.Add($missingKey)) {
                $ambiguities.Add([ordered]@{
                        kind          = 'macroCompileFixtureUnevidenced'
                        package       = $Fact.folder
                        trigger       = $Trigger
                        fixtures      = @($missing)
                        requiredInput = "macroContracts.$($Fact.folder).compileEvidence"
                    }) | Out-Null
            }
            $blocked = $true
        }
    }

    if (@($Contract.EvidenceIssues).Count -gt 0) {
        $inconclusiveKey = "$($Fact.folder)|macroCompileEvidenceInconclusive"
        if ($ambiguityKeys.Add($inconclusiveKey)) {
            $ambiguities.Add([ordered]@{
                    kind          = 'macroCompileEvidenceInconclusive'
                    package       = $Fact.folder
                    trigger       = $Trigger
                    issues        = @($Contract.EvidenceIssues)
                    requiredInput = "macroContracts.$($Fact.folder).compileEvidence"
                }) | Out-Null
        }
        $blocked = $true
    }

    # The verdict is a checked assertion, not a declaration. Measured outcomes
    # set a floor; a declared verdict may sit at or above it, never below.
    if (
        $script:ChangeTypeRank[$Contract.ChangeType] -lt
            $script:ChangeTypeRank[$Contract.DerivedFloor]
    ) {
        $underKey = "$($Fact.folder)|macroVerdictUnderclassified"
        if ($ambiguityKeys.Add($underKey)) {
            $ambiguities.Add([ordered]@{
                    kind             = 'macroVerdictUnderclassified'
                    package          = $Fact.folder
                    trigger          = $Trigger
                    declaredVerdict  = $Contract.Verdict
                    derivedVerdict   = ConvertTo-MacroVerdictName -ChangeType $Contract.DerivedFloor
                    decidingFixtures = @($Contract.DecidingFixtures)
                    requiredInput    = "macroContracts.$($Fact.folder).verdict"
                }) | Out-Null
        }
        $blocked = $true
    } elseif ($selectionDecisions.ContainsKey($Fact.folder)) {
        # A measured compile-contract change also has to be the reason the
        # package was selected, so a "behaviour fix" cannot carry a break.
        $decision = $selectionDecisions[$Fact.folder]
        $requiredReasons = switch ($Contract.DerivedFloor) {
            'breaking'     { @('breaking') }
            'non-breaking' { @('breaking', 'nonbreaking-api', 'behavior-fix') }
            default        { @() }
        }
        if ($requiredReasons.Count -gt 0) {
            $derivedName = ConvertTo-MacroVerdictName -ChangeType $Contract.DerivedFloor
            if ($decision.Decision -ne 'accept') {
                throw "Selection decision '$($Fact.folder)' declines a package whose compile evidence derives a '$derivedName' macro contract."
            }
            if ($decision.Reason -notin $requiredReasons) {
                throw "Selection reason '$($decision.Reason)' for '$($Fact.folder)' conflicts with the '$derivedName' macro contract derived from its compile evidence. Use $($requiredReasons -join ' or ')."
            }
        }
    }

    $usedMacroContracts[$Fact.folder] = $Contract
    if ($blocked) { return $null }
    return $Contract
}

function Require-MacroContract {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [AllowNull()]$TriggerFact,
        [Parameter(Mandatory = $true)][string]$Trigger
    )

    $scope = @(Get-MacroReviewScope `
            -Fact $Fact `
            -Facts $facts `
            -TriggerFact $TriggerFact)
    $contract = Get-MacroContract -Fact $Fact -Request $request
    $triggerFolder = if ($null -eq $TriggerFact) { '' } else { $TriggerFact.folder }
    $key = "$($Fact.folder)|$Trigger|$triggerFolder"
    if ($null -eq $contract) {
        if ($ambiguityKeys.Add($key)) {
            $ambiguities.Add([ordered]@{
                    kind          = 'macroContractUnreviewed'
                    package       = $Fact.folder
                    trigger       = $Trigger
                    reviewScope   = $scope
                    requiredInput = "macroContracts.$($Fact.folder)"
                }) | Out-Null
        }
        return $null
    }

    if (-not (Test-MacroContractCoversScope -Contract $contract -Scope $scope)) {
        if ($ambiguityKeys.Add($key)) {
            $ambiguities.Add([ordered]@{
                    kind          = 'macroContractIncomplete'
                    package       = $Fact.folder
                    trigger       = $Trigger
                    reviewScope   = $scope
                    reviewed      = @($contract.ReviewedPackages)
                    requiredInput = "macroContracts.$($Fact.folder).reviewedPackages"
                }) | Out-Null
        }
        return $null
    }

    if (
        $contract.Channels.generatedRuntimePaths.ToString().ToLowerInvariant() -eq 'changed' -and
        @($Fact.macroRuntimePartners | Where-Object { $_ }).Count -eq 0
    ) {
        $runtimeKey = "$($Fact.folder)|macroRuntimeUnknown"
        if ($ambiguityKeys.Add($runtimeKey)) {
            $ambiguities.Add([ordered]@{
                    kind          = 'macroRuntimeUnknown'
                    package       = $Fact.folder
                    trigger       = $Trigger
                    reviewScope   = $scope
                    requiredInput = 'Expose the macro from its runtime facade, emit a literal workspace path, or declare package.metadata.oxidizer_release.macro_runtime.'
                }) | Out-Null
        }
        return $null
    }

    return Register-MacroContract `
        -Fact $Fact `
        -Contract $contract `
        -Trigger $Trigger
}

function Write-BlockedPlan {
    [ordered]@{
        status         = 'blocked'
        mode           = $mode
        selectionDecisions = @(
            $selectionDecisions.GetEnumerator() |
                Sort-Object Key |
                ForEach-Object {
                    [ordered]@{
                        package  = $_.Key
                        decision = $_.Value.Decision
                        reason   = $_.Value.Reason
                        evidence = @($_.Value.Evidence)
                        regressionEvidence = @($_.Value.RegressionEvidence)
                    }
                }
        )
        releases       = @()
        macroContracts = @(
            $usedMacroContracts.GetEnumerator() |
                Sort-Object Key |
                ForEach-Object {
                    [ordered]@{
                        package  = $_.Key
                        verdict  = $_.Value.Verdict
                        derivedVerdict = ConvertTo-MacroVerdictName -ChangeType $_.Value.DerivedFloor
                        reviewed = @(Get-EmittedReviewScope -Folder $_.Key)
                        evidence = @($_.Value.Evidence)
                    }
                }
        )
        ambiguities    = @($ambiguities | Sort-Object package, kind, trigger)
        warnings       = @($warnings)
    } | ConvertTo-Json -Depth 10
}

# Selection evidence is graded before any token is expanded, so a plan whose
# reasons are not demonstrated can never reach the release loop.
foreach ($selectionFolder in @($selectionDecisions.Keys | Sort-Object)) {
    Register-SelectionEvidence `
        -Folder $selectionFolder `
        -Decision $selectionDecisions[$selectionFolder]
}

foreach ($tokenValue in $tokens) {
    $token = $tokenValue.ToString()
    $parts = $token -split '@', 2
    $fact = Find-PackageFact -Identifier $parts[0] -Facts $facts
    if (-not [bool]$fact.published) {
        throw "Package '$($fact.folder)' is not publishable."
    }
    if ($plan.ContainsKey($fact.folder)) {
        throw "Package '$($fact.folder)' appears more than once in the release tokens."
    }
    if (-not $tokenFolders.Add($fact.folder)) {
        throw "Package '$($fact.folder)' appears more than once in the release tokens."
    }
    if ($mode -in @('changed', 'all') -and -not $selectionDecisions.ContainsKey($fact.folder)) {
        throw "Release token '$($fact.folder)' is not a candidate in $mode mode."
    }
    if ($mode -in @('changed', 'all') -and $selectionDecisions[$fact.folder].Decision -ne 'accept') {
        throw "Release token '$($fact.folder)' conflicts with its decline selection decision."
    }

    $classification = Get-Classification -Fact $fact -Request $request
    Register-ExternalExposure -Fact $fact -ChangeType $classification.ChangeType
    Register-OwnDiffFloor -Fact $fact -ChangeType $classification.ChangeType
    $requestedChangeType = 'none'
    $requestedPin = $null
    if ($parts.Count -eq 2) {
        try {
            $requestedChangeType = ConvertTo-InternalChangeType -Value $parts[1]
        } catch {
            [void](Split-SemanticVersion -version $parts[1])
            $requestedPin = $parts[1]
            if ((Compare-SemanticVersions -version1 $requestedPin -version2 $fact.version) -le 0) {
                throw "Explicit pin '$requestedPin' for '$($fact.folder)' must be strictly greater than '$($fact.version)'."
            }
        }
    }

    $effectiveChangeType = Get-StrongerChangeType `
        -Left $classification.ChangeType `
        -Right $requestedChangeType
    if (-not [string]::IsNullOrWhiteSpace($requestedPin)) {
        $pinChangeType = Get-ChangeTypeFromVersions `
            -oldVersion $fact.version `
            -newVersion $requestedPin
        $effectiveChangeType = Get-StrongerChangeType `
            -Left $effectiveChangeType `
            -Right $pinChangeType
    }
    if ($effectiveChangeType -eq 'none') {
        $effectiveChangeType = 'patch'
    }

    $macroContract = Get-MacroContract -Fact $fact -Request $request
    if ([bool]$fact.procMacroOnly) {
        $modifiedScope = @(Get-ModifiedMacroScopeMember -Fact $fact)
        $needsMacroReview =
            [bool]$fact.modified -or
            $modifiedScope.Count -gt 0 -or
            $classification.ChangeType -ne 'patch' -or
            $requestedChangeType -in @('non-breaking', 'breaking') -or
            -not [string]::IsNullOrWhiteSpace($requestedPin)
        if ($needsMacroReview) {
            $trigger = if ([bool]$fact.modified) {
                'macroPackageModified'
            } elseif ($modifiedScope.Count -gt 0) {
                'implementationClosureModified'
            } else {
                'macroContractChangeRequested'
            }
            $macroContract = Require-MacroContract `
                -Fact $fact `
                -TriggerFact $null `
                -Trigger $trigger
            if ($null -eq $macroContract) { continue }
        } elseif ($null -ne $macroContract) {
            $macroContract = Register-MacroContract `
                -Fact $fact `
                -Contract $macroContract `
                -Trigger 'macroContractSupplied'
            if ($null -eq $macroContract) { continue }
        }
        if (
            $null -ne $macroContract -and
            $script:ChangeTypeRank[$requestedChangeType] -gt
                $script:ChangeTypeRank[$macroContract.ChangeType]
        ) {
            throw "Requested change '$requestedChangeType' for proc macro '$($fact.folder)' conflicts with its '$($macroContract.ChangeType)' contract verdict. Use an exact version pin for a compatible version-line change."
        }
    }

    $entry = [pscustomobject]@{
        Fact                = $fact
        Source              = 'user'
        RequestedPin        = $requestedPin
        EffectiveChangeType = $effectiveChangeType
        TargetVersion       = $null
        ManualReview        = [bool]$classification.ManualReview
        MacroContractReviewed = $null -ne $macroContract
        ContractBreaking    = [bool](
            [bool]$fact.procMacroOnly -and
            (
                ($null -ne $macroContract -and $macroContract.ChangeType -eq 'breaking') -or
                ($null -eq $macroContract -and
                    ($classification.ChangeType -eq 'breaking' -or $requestedChangeType -eq 'breaking'))
            )
        )
        Reasons             = @{}
    }
    Assert-PinSatisfiesRequirement `
        -Entry $entry `
        -RequiredChangeType $effectiveChangeType `
        -Force $force `
        -Warnings $warnings
    $entry.TargetVersion = Get-EntryTargetVersion -Entry $entry
    $plan[$fact.folder] = $entry
    $queue.Enqueue($fact.folder)
}

if ($mode -in @('changed', 'all')) {
    foreach ($candidate in $selectionDecisions.GetEnumerator()) {
        if (
            $candidate.Value.Decision -eq 'accept' -and
            -not $tokenFolders.Contains($candidate.Key)
        ) {
            throw "Accepted selection decision '$($candidate.Key)' is missing a release token."
        }
    }

    # A declined proc macro never reaches the token loop, so its compile-fixture
    # obligations would otherwise go unreviewed. Requiring the contract here
    # keeps the decline honest without forcing a release: a measured
    # fail -> fail outcome leaves the decline standing, while a measured break
    # cannot be declined at all.
    foreach ($candidate in $selectionDecisions.GetEnumerator()) {
        if ($candidate.Value.Decision -ne 'decline') { continue }
        $candidateFact = @($facts | Where-Object { $_.folder -eq $candidate.Key })[0]
        if ($null -eq $candidateFact -or -not [bool]$candidateFact.procMacroOnly) {
            continue
        }
        if (@($candidateFact.macroCompileFixtureChanges).Count -eq 0) { continue }
        [void](Require-MacroContract `
                -Fact $candidateFact `
                -TriggerFact $null `
                -Trigger 'macroCompileFixtureChanged')
    }
}

if ($ambiguities.Count -gt 0) {
    Write-BlockedPlan
    return
}

while ($queue.Count -gt 0) {
    $dependencyFolder = $queue.Dequeue()
    $dependencyEntry = $plan[$dependencyFolder]
    $dependencyName = $dependencyEntry.Fact.name.Replace('-', '_')
    $dependencyVersionBreaking =
        [bool]$dependencyEntry.Fact.everReleased -and
        (Compare-SemanticVersions `
            -version1 $dependencyEntry.TargetVersion `
            -version2 $dependencyEntry.Fact.version) -ne 0 -and
        (Test-IsBreakingChange `
            -oldVersion $dependencyEntry.Fact.version `
            -ChangeType $dependencyEntry.EffectiveChangeType)
    $dependencyContractBreaking =
        [bool]$dependencyEntry.Fact.procMacroOnly -and
        [bool]$dependencyEntry.ContractBreaking

    $dependents = @(
        $facts |
            Where-Object {
                [bool]$_.published -and
                [bool]$_.everReleased -and
                $_.folder -ne $dependencyFolder -and
                (
                    @($_.deps) -contains $dependencyName -or
                    (
                        -not [bool]$dependencyEntry.Fact.procMacroOnly -and
                        $dependencyVersionBreaking -and
                        @($_.exposedDeps) -contains $dependencyName
                    ) -or
                    (
                        [bool]$dependencyEntry.Fact.procMacroOnly -and
                        $dependencyContractBreaking -and
                        @($_.macroPublicDeps) -contains $dependencyName
                    )
                )
            } |
            Sort-Object folder
    )

    foreach ($dependentFact in $dependents) {
        $classification = Get-Classification -Fact $dependentFact -Request $request
        Register-ExternalExposure `
            -Fact $dependentFact `
            -ChangeType $classification.ChangeType
        Register-OwnDiffFloor `
            -Fact $dependentFact `
            -ChangeType $classification.ChangeType
        $macroContract = $null
        if ([bool]$dependentFact.procMacroOnly) {
            $modifiedScope = @(Get-ModifiedMacroScopeMember -Fact $dependentFact)
            $needsMacroReview =
                $dependencyVersionBreaking -or
                [bool]$dependentFact.modified -or
                $modifiedScope.Count -gt 0 -or
                $classification.ChangeType -ne 'patch'
            if ($needsMacroReview) {
                $macroContract = Require-MacroContract `
                    -Fact $dependentFact `
                    -TriggerFact $dependencyEntry.Fact `
                    -Trigger 'implementationDependencyChanged'
                if ($null -eq $macroContract) { continue }
            } else {
                $macroContract = Get-MacroContract `
                    -Fact $dependentFact `
                    -Request $request
                if ($null -ne $macroContract) {
                    $macroContract = Register-MacroContract `
                        -Fact $dependentFact `
                        -Contract $macroContract `
                        -Trigger 'macroContractSupplied'
                }
            }
        }

        $isDirectDependent = @($dependentFact.deps) -contains $dependencyName
        if ([bool]$dependentFact.procMacroOnly) {
            $edgeClass = 'macroImplementation'
            $edgeBreaking =
                $null -ne $macroContract -and
                $macroContract.ChangeType -eq 'breaking'
            $judgment = if ($edgeBreaking) {
                'contractBreaking'
            } elseif ($null -ne $macroContract) {
                'contractCompatible'
            } else {
                'patchFloor'
            }
            $judgmentSource = if ($null -ne $macroContract) {
                'macroContracts'
            } else {
                'dependencyRequirement'
            }
        } elseif ([bool]$dependencyEntry.Fact.procMacroOnly) {
            $macroIsPublic = @($dependentFact.macroPublicDeps) -contains $dependencyName
            $edgeClass = if ($macroIsPublic) { 'macroPublic' } else { 'macroPrivate' }
            $edgeBreaking = $dependencyContractBreaking -and $macroIsPublic
            $judgment = if ($edgeBreaking) {
                'contractBreaking'
            } elseif ($macroIsPublic -and [bool]$dependencyEntry.MacroContractReviewed) {
                'contractCompatible'
            } elseif ($macroIsPublic) {
                'patchFloor'
            } else {
                'privateDependency'
            }
            $judgmentSource = if ([bool]$dependencyEntry.MacroContractReviewed) {
                'macroContracts'
            } else {
                'dependencyRequirement'
            }
        } else {
            $exposesDependency =
                ($isDirectDependent -and [bool]$dependentFact.exposureUnknown) -or
                (@($dependentFact.exposedDeps) -contains $dependencyName)
            $edgeClass = 'type'
            $edgeBreaking = $dependencyVersionBreaking -and $exposesDependency
            $judgment = if ($edgeBreaking) { 'typeExposed' } else { 'encapsulated' }
            $judgmentSource = 'releaseFacts'
        }

        $cascadeChangeType = Get-StrongerChangeType `
            -Left 'patch' `
            -Right $classification.ChangeType
        if ($edgeBreaking) {
            $cascadeChangeType = 'breaking'
        }

        $isNew = -not $plan.ContainsKey($dependentFact.folder)
        if ($isNew) {
            $dependentEntry = [pscustomobject]@{
                Fact                = $dependentFact
                Source              = 'cascade'
                RequestedPin        = $null
                EffectiveChangeType = $cascadeChangeType
                TargetVersion       = $null
                ManualReview        = [bool]$classification.ManualReview
                MacroContractReviewed = $null -ne $macroContract
                ContractBreaking    = [bool](
                    [bool]$dependentFact.procMacroOnly -and
                    $null -ne $macroContract -and
                    $macroContract.ChangeType -eq 'breaking'
                )
                Reasons             = @{}
            }
            $dependentEntry.TargetVersion = Get-EntryTargetVersion -Entry $dependentEntry
            $plan[$dependentFact.folder] = $dependentEntry
        } else {
            $dependentEntry = $plan[$dependentFact.folder]
        }

        $dependentEntry.Reasons[$dependencyFolder] = [pscustomobject]@{
            Target   = $dependencyEntry.Fact.name
            Version  = $dependencyEntry.TargetVersion
            Breaking = [bool]$edgeBreaking
            EdgeClass = $edgeClass
            Judgment = $judgment
            JudgmentSource = $judgmentSource
        }

        $stronger = Get-StrongerChangeType `
            -Left $dependentEntry.EffectiveChangeType `
            -Right $cascadeChangeType
        $strengthened = $stronger -ne $dependentEntry.EffectiveChangeType
        if ($strengthened) {
            $dependentEntry.EffectiveChangeType = $stronger
            if (
                [bool]$dependentFact.procMacroOnly -and
                $null -ne $macroContract -and
                $macroContract.ChangeType -eq 'breaking'
            ) {
                $dependentEntry.ContractBreaking = $true
            }
            Assert-PinSatisfiesRequirement `
                -Entry $dependentEntry `
                -RequiredChangeType $stronger `
                -Force $force `
                -Warnings $warnings
            $dependentEntry.TargetVersion = Get-EntryTargetVersion -Entry $dependentEntry
        }

        if ($isNew -or $strengthened) {
            $queue.Enqueue($dependentFact.folder)
        }
    }

    if (-not [bool]$dependencyEntry.Fact.procMacroOnly -and $dependencyVersionBreaking) {
        $runtimeMacros = @(
            $facts |
                Where-Object {
                    [bool]$_.published -and
                    [bool]$_.everReleased -and
                    [bool]$_.procMacroOnly -and
                    @($_.macroRuntimePartners) -contains $dependencyName
                } |
                Sort-Object folder
        )
        foreach ($macroFact in $runtimeMacros) {
            $macroContract = Require-MacroContract `
                -Fact $macroFact `
                -TriggerFact $dependencyEntry.Fact `
                -Trigger 'generatedRuntimeChanged'
            if ($null -eq $macroContract -or $macroContract.ChangeType -eq 'patch') {
                continue
            }

            $classification = Get-Classification -Fact $macroFact -Request $request
            Register-ExternalExposure `
                -Fact $macroFact `
                -ChangeType $classification.ChangeType
            $cascadeChangeType = Get-StrongerChangeType `
                -Left $classification.ChangeType `
                -Right $macroContract.ChangeType
            $isNew = -not $plan.ContainsKey($macroFact.folder)
            if ($isNew) {
                $macroEntry = [pscustomobject]@{
                    Fact                = $macroFact
                    Source              = 'cascade'
                    RequestedPin        = $null
                    EffectiveChangeType = $cascadeChangeType
                    TargetVersion       = $null
                    ManualReview        = $true
                    MacroContractReviewed = $true
                    ContractBreaking    = $macroContract.ChangeType -eq 'breaking'
                    Reasons             = @{}
                }
                $macroEntry.TargetVersion = Get-EntryTargetVersion -Entry $macroEntry
                $plan[$macroFact.folder] = $macroEntry
            } else {
                $macroEntry = $plan[$macroFact.folder]
            }

            $macroEntry.Reasons[$dependencyFolder] = [pscustomobject]@{
                Target         = $dependencyEntry.Fact.name
                Version        = $dependencyEntry.TargetVersion
                Breaking       = $macroContract.ChangeType -eq 'breaking'
                EdgeClass      = 'macroRuntime'
                Judgment       = if ($macroContract.ChangeType -eq 'breaking') {
                    'contractBreaking'
                } else {
                    'contractNonbreaking'
                }
                JudgmentSource = 'macroContracts'
            }

            $stronger = Get-StrongerChangeType `
                -Left $macroEntry.EffectiveChangeType `
                -Right $cascadeChangeType
            $strengthened = $stronger -ne $macroEntry.EffectiveChangeType
            if ($strengthened) {
                $macroEntry.EffectiveChangeType = $stronger
                $macroEntry.ContractBreaking = $macroContract.ChangeType -eq 'breaking'
                $macroEntry.TargetVersion = Get-EntryTargetVersion -Entry $macroEntry
            }
            if ($isNew -or $strengthened) {
                $queue.Enqueue($macroFact.folder)
            }
        }
    }
}

if ($ambiguities.Count -gt 0) {
    Write-BlockedPlan
    return
}

$indegree = @{}
foreach ($folder in $plan.Keys) {
    $indegree[$folder] = 0
}
foreach ($folder in $plan.Keys) {
    foreach ($dependencyName in @($plan[$folder].Fact.deps | Sort-Object -Unique)) {
        $dependency = $plan.Values |
            Where-Object { $_.Fact.name.Replace('-', '_') -eq $dependencyName } |
            Select-Object -First 1
        if ($null -ne $dependency) {
            $indegree[$folder]++
        }
    }
}

$orderedFolders = New-Object 'System.Collections.Generic.List[string]'
while ($orderedFolders.Count -lt $plan.Count) {
    $ready = @(
        $indegree.Keys |
            Where-Object { $indegree[$_] -eq 0 -and -not $orderedFolders.Contains($_) } |
            Sort-Object
    )
    if ($ready.Count -eq 0) {
        throw 'The release set contains a dependency cycle and cannot be topologically ordered.'
    }

    foreach ($folder in $ready) {
        $orderedFolders.Add($folder) | Out-Null
        $releasedName = $plan[$folder].Fact.name.Replace('-', '_')
        foreach ($candidate in $plan.Keys) {
            if ($orderedFolders.Contains($candidate)) { continue }
            if (@($plan[$candidate].Fact.deps) -contains $releasedName) {
                $indegree[$candidate]--
            }
        }
    }
}

$releases = foreach ($folder in $orderedFolders) {
    $entry = $plan[$folder]
    [ordered]@{
        folder         = $entry.Fact.folder
        name           = $entry.Fact.name
        from           = $entry.Fact.version
        to             = $entry.TargetVersion
        changeType     = $entry.EffectiveChangeType.Replace('-', '')
        source         = $entry.Source
        manualReview   = [bool]$entry.ManualReview
        contractBreaking = [bool]$entry.ContractBreaking
        cascadeReasons = @(
            $entry.Reasons.Values |
                Sort-Object Target |
                ForEach-Object {
                    [ordered]@{
                        target   = $_.Target
                        version  = $_.Version
                        breaking = [bool]$_.Breaking
                        edgeClass = $_.EdgeClass
                        judgment = $_.Judgment
                        judgmentSource = $_.JudgmentSource
                    }
                }
        )
    }
}

[ordered]@{
    status         = 'resolved'
    mode           = $mode
    selectionDecisions = @(
        $selectionDecisions.GetEnumerator() |
            Sort-Object Key |
            ForEach-Object {
                [ordered]@{
                    package  = $_.Key
                    decision = $_.Value.Decision
                    reason   = $_.Value.Reason
                    evidence = @($_.Value.Evidence)
                    regressionEvidence = @($_.Value.RegressionEvidence)
                }
            }
    )
    releases       = @($releases)
    macroContracts = @(
        $usedMacroContracts.GetEnumerator() |
            Sort-Object Key |
            ForEach-Object {
                [ordered]@{
                    package  = $_.Key
                    verdict  = $_.Value.Verdict
                    derivedVerdict = ConvertTo-MacroVerdictName -ChangeType $_.Value.DerivedFloor
                    reviewed = @(Get-EmittedReviewScope -Folder $_.Key)
                    evidence = @($_.Value.Evidence)
                }
            }
    )
    ambiguities    = @()
    warnings       = @($warnings)
} | ConvertTo-Json -Depth 8
