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
    JSON with mode, tokens, classifications, macroContracts, and optional force:
      {
        "mode": "targeted",
        "tokens": ["bytesbuf@breaking"],
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
            "evidence": ["Expansion snapshots and compile fixtures are unchanged."]
          }
        },
        "force": false
      }
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

        return [pscustomobject]@{
            Verdict         = $verdict.ToString().ToLowerInvariant().Replace('-', '')
            ChangeType      = $changeType
            ReviewedPackages = $reviewedPackages
            Channels        = $value.channels
            Evidence        = $evidence
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
        if ($null -ne $value.manualReview) {
            $manualReview = [bool]$value.manualReview
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
if ($factsDocument.schemaVersion -ne 2) {
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

$tokens = @($request.tokens)
if ($tokens.Count -eq 0) {
    throw "Release mode '$mode' requires at least one accepted package token."
}

$force = [bool]$request.force
$warnings = New-Object 'System.Collections.Generic.List[string]'
$ambiguities = New-Object 'System.Collections.Generic.List[object]'
$ambiguityKeys = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
$usedMacroContracts = @{}
$plan = @{}
$queue = New-Object 'System.Collections.Generic.Queue[string]'

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

    $usedMacroContracts[$Fact.folder] = $contract
    return $contract
}

function Write-BlockedPlan {
    [ordered]@{
        status         = 'blocked'
        mode           = $mode
        releases       = @()
        macroContracts = @(
            $usedMacroContracts.GetEnumerator() |
                Sort-Object Key |
                ForEach-Object {
                    [ordered]@{
                        package  = $_.Key
                        verdict  = $_.Value.Verdict
                        reviewed = @($_.Value.ReviewedPackages)
                        evidence = @($_.Value.Evidence)
                    }
                }
        )
        ambiguities    = @($ambiguities | Sort-Object package, kind, trigger)
        warnings       = @($warnings)
    } | ConvertTo-Json -Depth 10
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

    $classification = Get-Classification -Fact $fact -Request $request
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
            $usedMacroContracts[$fact.folder] = $macroContract
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
                    $usedMacroContracts[$dependentFact.folder] = $macroContract
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
    releases       = @($releases)
    macroContracts = @(
        $usedMacroContracts.GetEnumerator() |
            Sort-Object Key |
            ForEach-Object {
                [ordered]@{
                    package  = $_.Key
                    verdict  = $_.Value.Verdict
                    reviewed = @($_.Value.ReviewedPackages)
                    evidence = @($_.Value.Evidence)
                }
            }
    )
    ambiguities    = @()
    warnings       = @($warnings)
} | ConvertTo-Json -Depth 8
