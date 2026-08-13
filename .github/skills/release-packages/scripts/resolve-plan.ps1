# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Resolves a deterministic release plan from facts and model classifications.

.DESCRIPTION
    Performs only mechanical work: token parsing, version arithmetic, dependency
    closure, exposure-aware breaking propagation, pin reconciliation, and
    topological ordering. The release skill remains responsible for classifying
    source diffs and reviewing proc-macro behavior.

.PARAMETER FactsPath
    JSON emitted by release-facts.ps1.

.PARAMETER RequestPath
    JSON with mode, tokens, classifications, and optional force:
      {
        "mode": "targeted",
        "tokens": ["bytesbuf@breaking"],
        "classifications": {
          "bytesbuf": "patch",
          "bytesbuf_io": { "changeType": "patch", "manualReview": false }
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

function Get-Classification {
    param(
        [Parameter(Mandatory = $true)]$Fact,
        [Parameter(Mandatory = $true)]$Request
    )

    $value = $null
    if ($null -ne $Request.classifications) {
        $property = $Request.classifications.PSObject.Properties[$Fact.folder]
        if ($null -eq $property) {
            $property = $Request.classifications.PSObject.Properties[$Fact.name]
        }
        if ($null -ne $property) {
            $value = $property.Value
        }
    }

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
$facts = @($factsDocument.packages)
if ($facts.Count -eq 0) {
    throw 'The facts document contains no workspace packages.'
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
$plan = @{}
$queue = New-Object 'System.Collections.Generic.Queue[string]'

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

    $entry = [pscustomobject]@{
        Fact                = $fact
        Source              = 'user'
        RequestedPin        = $requestedPin
        EffectiveChangeType = $effectiveChangeType
        TargetVersion       = $null
        ManualReview        = [bool]$classification.ManualReview
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

while ($queue.Count -gt 0) {
    $dependencyFolder = $queue.Dequeue()
    $dependencyEntry = $plan[$dependencyFolder]
    $dependencyName = $dependencyEntry.Fact.name.Replace('-', '_')
    $dependencyBreaksConsumers =
        [bool]$dependencyEntry.Fact.everReleased -and
        (Compare-SemanticVersions `
            -version1 $dependencyEntry.TargetVersion `
            -version2 $dependencyEntry.Fact.version) -ne 0 -and
        (Test-IsBreakingChange `
            -oldVersion $dependencyEntry.Fact.version `
            -ChangeType $dependencyEntry.EffectiveChangeType)

    $dependents = @(
        $facts |
            Where-Object {
                [bool]$_.published -and
                [bool]$_.everReleased -and
                $_.folder -ne $dependencyFolder -and
                @($_.deps) -contains $dependencyName
            } |
            Sort-Object folder
    )

    foreach ($dependentFact in $dependents) {
        $classification = Get-Classification -Fact $dependentFact -Request $request
        $exposesDependency =
            [bool]$dependentFact.exposureUnknown -or
            (@($dependentFact.exposedDeps) -contains $dependencyName)
        $edgeBreaking = $dependencyBreaksConsumers -and $exposesDependency
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
        }

        $stronger = Get-StrongerChangeType `
            -Left $dependentEntry.EffectiveChangeType `
            -Right $cascadeChangeType
        $strengthened = $stronger -ne $dependentEntry.EffectiveChangeType
        if ($strengthened) {
            $dependentEntry.EffectiveChangeType = $stronger
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
        cascadeReasons = @(
            $entry.Reasons.Values |
                Sort-Object Target |
                ForEach-Object {
                    [ordered]@{
                        target   = $_.Target
                        version  = $_.Version
                        breaking = [bool]$_.Breaking
                    }
                }
        )
    }
}

[ordered]@{
    mode     = $mode
    releases = @($releases)
    warnings = @($warnings)
} | ConvertTo-Json -Depth 8
