# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')

    $script:Resolver = Join-Path (
        Get-OxiRepoRoot
    ) '.github\skills\release-packages\scripts\resolve-plan.ps1'

    function New-ReleaseFact {
        param(
            [Parameter(Mandatory = $true)][string]$Name,
            [string]$Version = '1.0.0',
            [string[]]$Deps = @(),
            [string[]]$ExposedDeps = @(),
            [string[]]$MacroPublicDeps = @(),
            [string[]]$MacroImplementationClosure = @(),
            [string[]]$MacroRuntimePartners = @(),
            [bool]$ExposureUnknown = $false,
            [bool]$Published = $true,
            [bool]$EverReleased = $true,
            [bool]$ProcMacroOnly = $false,
            [bool]$Modified = $true,
            [string[]]$ModifiedFiles = @(),
            [string[]]$ManifestDependencyScopes = @(),
            [bool]$ManifestOtherChanged = $false,
            [object[]]$MacroCompileFixtureChanges = @(),
            [object[]]$ExternalDepChanges = @(),
            [string[]]$ExternalExposedDeps = @(),
            [bool]$RustImplementationChanged = $true,
            [bool]$DocCommentChanged = $false,
            [bool]$WorkspaceModified = $Modified
        )

        if ($Modified -and $ModifiedFiles.Count -eq 0) {
            $ModifiedFiles = @("crates/$Name/src/lib.rs")
        }

        return [ordered]@{
            folder           = $Name
            name             = $Name
            version          = $Version
            published        = $Published
            procMacroOnly    = $ProcMacroOnly
            hasLibraryTarget = -not $ProcMacroOnly
            deps             = @($Deps)
            exposedDeps      = @($ExposedDeps)
            macroPublicDeps  = @($MacroPublicDeps)
            macroImplementationClosure = @($MacroImplementationClosure)
            macroRuntimePartners = @($MacroRuntimePartners)
            exposureUnknown  = $ExposureUnknown
            baselineSha      = if ($EverReleased) { '0123456789012345678901234567890123456789' } else { $null }
            hasBaseline      = $EverReleased
            everReleased     = $EverReleased
            modified         = $Modified
            modifiedFiles    = @($ModifiedFiles)
            modifiedFileCount = $ModifiedFiles.Count
            manifestDependencyScopes = @($ManifestDependencyScopes)
            manifestOtherChanged = $ManifestOtherChanged
            rustImplementationChanged = $RustImplementationChanged
            docCommentChanged = $DocCommentChanged
            macroCompileFixtureChanges = @($MacroCompileFixtureChanges)
            externalDepChanges = @($ExternalDepChanges)
            externalExposedDeps = @($ExternalExposedDeps)
            workspaceModified = $WorkspaceModified
        }
    }

    # A fact-side external dependency requirement change, shaped exactly as
    # release-facts.ps1 emits it.
    function New-ExternalDepChange {
        param(
            [Parameter(Mandatory = $true)][string]$Name,
            [AllowNull()][string]$BaselineReq = '^2.0.111',
            [AllowNull()][string]$CurrentReq = '^3.0.2',
            [string[]]$Kinds = @('normal'),
            [bool]$Breaking = $true,
            [string]$BaselineRev = '0123456789012345678901234567890123456789'
        )

        return [ordered]@{
            name        = $Name
            baselineReq = $BaselineReq
            currentReq  = $CurrentReq
            kinds       = @($Kinds)
            breaking    = $Breaking
            baselineRev = $BaselineRev
        }
    }

    # A fact-side compile-fixture obligation, shaped exactly as release-facts.ps1
    # emits it.
    function New-CompileFixtureChange {
        param(
            [Parameter(Mandatory = $true)][string]$OwnerPackage,
            [Parameter(Mandatory = $true)][string]$Path,
            [ValidateSet('added', 'modified', 'removed')]
            [string]$Status = 'added',
            [ValidateSet('self', 'runtimePartner', 'implementationClosure')]
            [string]$ScopeRole = 'runtimePartner',
            [bool]$OwnerPublished = $true,
            [AllowNull()][string]$ExpectedResult = 'fail',
            [string]$BaselineRev = '0123456789012345678901234567890123456789'
        )

        return [ordered]@{
            ownerPackage   = $OwnerPackage
            ownerPublished = $OwnerPublished
            path           = $Path
            kind           = if ($Path.EndsWith('.rs')) { 'uiFixture' } else { 'uiExpectation' }
            status         = $Status
            expectedResult = $ExpectedResult
            baselineRev    = $BaselineRev
            scopeRole      = $ScopeRole
        }
    }

    # A contract-side measurement of one fixture, keyed to the obligation above.
    function New-CompileEvidence {
        param(
            [Parameter(Mandatory = $true)][string]$OwnerPackage,
            [Parameter(Mandatory = $true)][string]$Path,
            [ValidateSet('pass', 'fail')][string]$Baseline = 'pass',
            [ValidateSet('pass', 'fail')][string]$Current = 'pass',
            [string]$BaselineRev = '0123456789012345678901234567890123456789',
            [string]$CurrentRev = 'abcdefabcdefabcdefabcdefabcdefabcdefabcd'
        )

        return @{
            ownerPackage = $OwnerPackage
            path         = $Path
            baseline     = @{
                result   = $Baseline
                revision = $BaselineRev
                exitCode = if ($Baseline -eq 'pass') { 0 } else { 101 }
            }
            current      = @{
                result   = $Current
                revision = $CurrentRev
                exitCode = if ($Current -eq 'pass') { 0 } else { 101 }
            }
        }
    }

    function New-MacroContract {
        param(
            [ValidateSet('compatible', 'nonbreaking', 'breaking')]
            [string]$Verdict = 'compatible',
            [string[]]$ReviewedPackages = @('macros'),
            [string[]]$Evidence = @('Reviewed macro exports, compile fixtures, and generated API.'),
            [object[]]$CompileEvidence = @()
        )

        $contract = @{
            verdict = $Verdict
            reviewedPackages = @($ReviewedPackages)
            channels = @{
                exportedMacros = 'unchanged'
                acceptedSyntax = 'unchanged'
                compileBehavior = 'unchanged'
                generatedApi = 'unchanged'
                generatedRuntimePaths = 'unchanged'
                hygiene = 'unchanged'
            }
            evidence = @($Evidence)
        }
        if ($CompileEvidence.Count -gt 0) {
            $contract['compileEvidence'] = @($CompileEvidence)
        }
        return $contract
    }

    function Invoke-ReleasePlan {
        param(
            [Parameter(Mandatory = $true)][object[]]$Facts,
            [Parameter(Mandatory = $true)][hashtable]$Request,
            [int]$SchemaVersion = 5
        )

        $caseDir = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $caseDir | Out-Null
        $factsPath = Join-Path $caseDir 'facts.json'
        $requestPath = Join-Path $caseDir 'request.json'
        [ordered]@{ schemaVersion = $SchemaVersion; packages = @($Facts) } |
            ConvertTo-Json -Depth 8 |
            Set-Content -LiteralPath $factsPath -Encoding utf8
        $Request |
            ConvertTo-Json -Depth 8 |
            Set-Content -LiteralPath $requestPath -Encoding utf8

        $json = & $script:Resolver `
            -FactsPath $factsPath `
            -RequestPath $requestPath
        return ($json | ConvertFrom-Json)
    }

    function New-RegressionEvidence {
        param(
            [string]$Kind = 'consumer-runtime',
            [string]$Probe = 'cargo test -p package --test regression',
            [string]$Baseline = 'fail',
            [string]$Current = 'pass',
            [string]$BaselineRevision = 'baseline-sha',
            [string]$CurrentRevision = 'worktree',
            [object]$BaselineExitCode,
            [object]$CurrentExitCode
        )

        $baselineExit = if ($PSBoundParameters.ContainsKey('BaselineExitCode')) {
            $BaselineExitCode
        } elseif ($Baseline -eq 'pass') { 0 } else { 101 }
        $currentExit = if ($PSBoundParameters.ContainsKey('CurrentExitCode')) {
            $CurrentExitCode
        } elseif ($Current -eq 'pass') { 0 } else { 101 }

        return @{
            kind = $Kind
            probe = $Probe
            baseline = @{
                revision = $BaselineRevision
                result = $Baseline
                exitCode = $baselineExit
            }
            current = @{
                revision = $CurrentRevision
                result = $Current
                exitCode = $currentExit
            }
        }
    }

    function New-SelectionDecision {
        param(
            [ValidateSet('accept', 'decline')]
            [string]$Decision = 'accept',
            [string]$Reason = 'behavior-fix',
            [AllowNull()][AllowEmptyCollection()][object[]]$RegressionEvidence
        )

        $value = @{
            decision = $Decision
            reason = $Reason
            evidence = @('Reviewed the package diff from its release baseline.')
        }
        if ($PSBoundParameters.ContainsKey('RegressionEvidence')) {
            $value.regressionEvidence = @($RegressionEvidence)
        } elseif ($Reason -eq 'behavior-fix') {
            $value.regressionEvidence = @(New-RegressionEvidence)
        }
        return $value
    }
}

Describe 'resolve-plan.ps1 version arithmetic' {
    BeforeDiscovery {
        $cases = @(
            @{ Name = 'stable breaking'; Version = '1.2.3'; Change = 'breaking'; Expected = '2.0.0' }
            @{ Name = 'stable nonbreaking'; Version = '1.2.3'; Change = 'nonbreaking'; Expected = '1.3.0' }
            @{ Name = 'stable patch'; Version = '1.2.3'; Change = 'patch'; Expected = '1.2.4' }
            @{ Name = '0.x breaking'; Version = '0.4.2'; Change = 'breaking'; Expected = '0.5.0' }
            @{ Name = '0.x nonbreaking'; Version = '0.4.2'; Change = 'nonbreaking'; Expected = '0.4.3' }
            @{ Name = '0.x patch'; Version = '0.4.2'; Change = 'patch'; Expected = '0.4.3' }
            @{ Name = '0.0.x breaking'; Version = '0.0.5'; Change = 'breaking'; Expected = '0.0.6' }
            @{ Name = '0.0.x nonbreaking'; Version = '0.0.5'; Change = 'nonbreaking'; Expected = '0.0.6' }
            @{ Name = '0.0.x patch'; Version = '0.0.5'; Change = 'patch'; Expected = '0.0.6' }
        )
    }

    It '<Name>' -ForEach $cases {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package -Version $Version) `
            -Request @{
                mode = 'targeted'
                tokens = @("package@$Change")
                classifications = @{ package = 'patch' }
            }

        $plan.releases.Count | Should -Be 1
        $plan.releases[0].to | Should -Be $Expected
        $plan.releases[0].changeType | Should -Be $Change
    }

    It 'ships a first-ever release at its declared version' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package -Version '0.1.0' -EverReleased $false) `
            -Request @{ mode = 'targeted'; tokens = @('package'); classifications = @{} }

        $plan.releases[0].from | Should -Be '0.1.0'
        $plan.releases[0].to | Should -Be '0.1.0'
    }

    It 'honors an explicit pin for a first-ever release' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package -Version '0.1.0' -EverReleased $false) `
            -Request @{ mode = 'targeted'; tokens = @('package@1.0.0'); classifications = @{} }

        $plan.releases[0].to | Should -Be '1.0.0'
    }

    It 'uses an objective classification stronger than the requested lower bound' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package -Version '1.2.3') `
            -Request @{
                mode = 'targeted'
                tokens = @('package@patch')
                classifications = @{ package = 'breaking' }
            }

        $plan.releases[0].to | Should -Be '2.0.0'
        $plan.releases[0].changeType | Should -Be 'breaking'
    }
}

Describe 'resolve-plan.ps1 cascades' {
    It 'cascades patch through a non-exposing linear chain' {
        $facts = @(
            New-ReleaseFact -Name bottom
            New-ReleaseFact -Name middle -Deps bottom
            New-ReleaseFact -Name top -Deps middle
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('bottom@patch')
            classifications = @{ bottom = 'patch'; middle = 'patch'; top = 'patch' }
        }

        @($plan.releases.folder) | Should -Be @('bottom', 'middle', 'top')
        @($plan.releases.to) | Should -Be @('1.0.1', '1.0.1', '1.0.1')
        @($plan.releases.changeType) | Should -Be @('patch', 'patch', 'patch')
    }

    It 'propagates a breaking release across exposure edges to a fixed point' {
        $facts = @(
            New-ReleaseFact -Name bottom
            New-ReleaseFact -Name middle -Version '0.3.2' -Deps bottom -ExposedDeps bottom
            New-ReleaseFact -Name top -Version '2.4.0' -Deps middle -ExposedDeps middle
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('bottom@breaking')
            classifications = @{ bottom = 'patch'; middle = 'patch'; top = 'patch' }
        }

        @($plan.releases.to) | Should -Be @('2.0.0', '0.4.0', '3.0.0')
        @($plan.releases.changeType) | Should -Be @('breaking', 'breaking', 'breaking')
        $plan.releases[1].cascadeReasons[0].breaking | Should -BeTrue
        $plan.releases[2].cascadeReasons[0].breaking | Should -BeTrue
    }

    It 'propagates a break to an indirect dependent that exposes the defining crate' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name relay -Deps core
            New-ReleaseFact -Name facade -Deps relay -ExposedDeps core
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch'; relay = 'patch'; facade = 'patch' }
        }

        @($plan.releases.folder) | Should -Be @('core', 'relay', 'facade')
        ($plan.releases | Where-Object folder -eq relay).changeType | Should -Be 'patch'
        $facade = $plan.releases | Where-Object folder -eq facade
        $facade.changeType | Should -Be 'breaking'
        $facade.cascadeReasons[0].target | Should -Be 'core'
        $facade.cascadeReasons[0].breaking | Should -BeTrue
    }

    It 'treats every 0.0.z bump as breaking when the dependency is exposed' {
        $facts = @(
            New-ReleaseFact -Name unstable -Version '0.0.5'
            New-ReleaseFact -Name consumer -Version '1.4.0' -Deps unstable -ExposedDeps unstable
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('unstable@patch')
            classifications = @{ unstable = 'patch'; consumer = 'patch' }
        }

        $plan.releases[0].to | Should -Be '0.0.6'
        $plan.releases[1].to | Should -Be '2.0.0'
        $plan.releases[1].cascadeReasons[0].breaking | Should -BeTrue
    }

    It 'merges diamond reasons and applies the strongest path once' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name left -Deps core -ExposedDeps core
            New-ReleaseFact -Name right -Deps core
            New-ReleaseFact -Name top -Deps @('left', 'right') -ExposedDeps left
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch'; left = 'patch'; right = 'patch'; top = 'patch' }
        }

        @($plan.releases.folder) | Should -Be @('core', 'left', 'right', 'top')
        ($plan.releases | Where-Object folder -eq left).changeType | Should -Be 'breaking'
        ($plan.releases | Where-Object folder -eq right).changeType | Should -Be 'patch'
        $top = $plan.releases | Where-Object folder -eq top
        $top.changeType | Should -Be 'breaking'
        @($top.cascadeReasons).Count | Should -Be 2
        @($top.cascadeReasons.target) | Should -Be @('left', 'right')
    }

    It 'skips unpublished dependents' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name private_consumer -Deps core -Published $false
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@patch')
            classifications = @{ core = 'patch' }
        }

        @($plan.releases.folder) | Should -Be @('core')
    }

    It 'blocks a breaking implementation dependency until the macro contract is reviewed' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps core `
                -MacroImplementationClosure core -ProcMacroOnly $true
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch' }
        }

        $plan.status | Should -Be 'blocked'
        $plan.releases.Count | Should -Be 0
        $plan.ambiguities[0].kind | Should -Be 'macroContractUnreviewed'
        @($plan.ambiguities[0].reviewScope) | Should -Be @('core', 'macros')
    }

    It 'keeps an implementation break at a proc-macro patch floor when its contract is compatible' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps core `
                -MacroImplementationClosure core -ProcMacroOnly $true
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -ReviewedPackages @('core', 'macros')
            }
        }

        $macros = $plan.releases | Where-Object folder -eq macros
        $macros.to | Should -Be '0.4.1'
        $macros.changeType | Should -Be 'patch'
        $macros.manualReview | Should -BeTrue
        $macros.contractBreaking | Should -BeFalse
        $macros.cascadeReasons[0].edgeClass | Should -Be 'macroImplementation'
        $macros.cascadeReasons[0].judgment | Should -Be 'contractCompatible'
    }

    It 'blocks an incomplete macro review scope' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps core `
                -MacroImplementationClosure core -ProcMacroOnly $true
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -ReviewedPackages @('macros')
            }
        }

        $plan.status | Should -Be 'blocked'
        $plan.ambiguities[0].kind | Should -Be 'macroContractIncomplete'
        @($plan.ambiguities[0].reviewScope) | Should -Contain 'core'
    }

    It 'blocks when an unpublished implementation helper changed' {
        $facts = @(
            New-ReleaseFact -Name helper -Published $false -Modified $false `
                -WorkspaceModified $true
            New-ReleaseFact -Name macros -ProcMacroOnly $true -Modified $false `
                -WorkspaceModified $false -MacroImplementationClosure helper
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{}
        }

        $plan.status | Should -Be 'blocked'
        @($plan.ambiguities[0].reviewScope) | Should -Be @('helper', 'macros')
    }

    It 'reviews an unpublished helper without releasing it' {
        $facts = @(
            New-ReleaseFact -Name helper -Published $false -Modified $false `
                -WorkspaceModified $true
            New-ReleaseFact -Name macros -ProcMacroOnly $true -Modified $false `
                -WorkspaceModified $false -MacroImplementationClosure helper
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{}
            macroContracts = @{
                macros = New-MacroContract -ReviewedPackages @('helper', 'macros')
            }
        }

        $plan.status | Should -Be 'resolved'
        @($plan.releases.folder) | Should -Be @('macros')
    }

    It 'blocks changed generated-runtime paths when no partner can be inferred' {
        $contract = New-MacroContract
        $contract.channels.generatedRuntimePaths = 'changed'
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name macros -Version '1.0.0' `
                    -ProcMacroOnly $true
            ) `
            -Request @{
                mode = 'targeted'
                tokens = @('macros@patch')
                classifications = @{}
                macroContracts = @{ macros = $contract }
            }

        $plan.status | Should -Be 'blocked'
        $plan.ambiguities[0].kind | Should -Be 'macroRuntimeUnknown'
    }

    It 'propagates a reviewed breaking macro contract through a public macro edge' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps core `
                -MacroImplementationClosure core -ProcMacroOnly $true
            New-ReleaseFact -Name runtime -Version '0.4.0' -Deps macros `
                -MacroPublicDeps macros
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict breaking `
                    -ReviewedPackages @('core', 'macros')
            }
        }

        $macros = $plan.releases | Where-Object folder -eq macros
        $runtime = $plan.releases | Where-Object folder -eq runtime
        $macros.to | Should -Be '0.5.0'
        $macros.contractBreaking | Should -BeTrue
        $runtime.to | Should -Be '0.5.0'
        $runtime.cascadeReasons[0].edgeClass | Should -Be 'macroPublic'
        $runtime.cascadeReasons[0].breaking | Should -BeTrue
    }

    It 'does not turn a compatible macro major pin into a public contract break' {
        $facts = @(
            New-ReleaseFact -Name macros -Version '0.4.0' -ProcMacroOnly $true
            New-ReleaseFact -Name runtime -Version '0.4.0' -Deps macros `
                -MacroPublicDeps macros
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros@0.5.0')
            classifications = @{ runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract
            }
        }

        $macros = $plan.releases | Where-Object folder -eq macros
        $runtime = $plan.releases | Where-Object folder -eq runtime
        $macros.changeType | Should -Be 'breaking'
        $macros.contractBreaking | Should -BeFalse
        $runtime.changeType | Should -Be 'patch'
        $runtime.cascadeReasons[0].edgeClass | Should -Be 'macroPublic'
        $runtime.cascadeReasons[0].breaking | Should -BeFalse
    }

    It 'requires a macro contract for an unchanged exact version pin' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name macros -Version '1.0.0' `
                    -ProcMacroOnly $true -Modified $false
            ) `
            -Request @{
                mode = 'targeted'
                tokens = @('macros@2.0.0')
                classifications = @{}
            }

        $plan.status | Should -Be 'blocked'
        $plan.ambiguities[0].kind | Should -Be 'macroContractUnreviewed'
    }

    It 'does not propagate Cargo breaking arithmetic for a compatible 0.0 proc macro' {
        $facts = @(
            New-ReleaseFact -Name macros -Version '0.0.5' -ProcMacroOnly $true
            New-ReleaseFact -Name runtime -Version '1.0.0' -Deps macros `
                -MacroPublicDeps macros
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract
            }
        }

        ($plan.releases | Where-Object folder -eq macros).to |
            Should -Be '0.0.6'
        ($plan.releases | Where-Object folder -eq runtime).changeType |
            Should -Be 'patch'
    }

    It 'keeps an internally used proc macro at a patch floor even when its contract breaks' {
        $facts = @(
            New-ReleaseFact -Name macros -Version '1.0.0' -ProcMacroOnly $true
            New-ReleaseFact -Name internal_user -Version '1.0.0' -Deps macros
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros@breaking')
            classifications = @{ internal_user = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict breaking
            }
        }

        $internal = $plan.releases | Where-Object folder -eq internal_user
        $internal.changeType | Should -Be 'patch'
        $internal.cascadeReasons[0].edgeClass | Should -Be 'macroPrivate'
        $internal.cascadeReasons[0].breaking | Should -BeFalse
    }

    It 'requires a contract when a cascade-reached macro is classified above patch' {
        $facts = @(
            New-ReleaseFact -Name core -Modified $false
            New-ReleaseFact -Name macros -Deps core -ProcMacroOnly $true `
                -Modified $false
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@patch')
            classifications = @{ core = 'patch'; macros = 'breaking' }
        }

        $plan.status | Should -Be 'blocked'
        $plan.ambiguities[0].kind | Should -Be 'macroContractUnreviewed'
    }

    It 'reviews generated-runtime coupling and releases the macro only for a changed contract' {
        $facts = @(
            New-ReleaseFact -Name macros -Version '1.0.0' -ProcMacroOnly $true `
                -MacroRuntimePartners runtime
            New-ReleaseFact -Name runtime -Version '1.0.0' -Deps macros
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('runtime@breaking')
            classifications = @{ runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict nonbreaking `
                    -ReviewedPackages @('macros', 'runtime')
            }
        }

        $macros = $plan.releases | Where-Object folder -eq macros
        $macros.changeType | Should -Be 'nonbreaking'
        $macros.cascadeReasons[0].edgeClass | Should -Be 'macroRuntime'
    }

    It 'uses exposureUnknown as a conservative breaking edge' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name consumer -Version '1.0.0' -Deps core -ExposureUnknown $true
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch'; consumer = 'patch' }
        }

        ($plan.releases | Where-Object folder -eq consumer).to | Should -Be '2.0.0'
    }

    It 'does not cascade into a package that has never been released' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name future -Deps core -EverReleased $false
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@patch')
            classifications = @{ core = 'patch' }
        }

        @($plan.releases.folder) | Should -Be @('core')
    }

    It 'deduplicates dependency edges while ordering' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name consumer -Deps @('core', 'core')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@patch')
            classifications = @{ core = 'patch'; consumer = 'patch' }
        }

        @($plan.releases.folder) | Should -Be @('core', 'consumer')
    }
}

Describe 'resolve-plan.ps1 macro compile evidence' {
    BeforeAll {
        # The run-8 shape: a proc macro that newly rejects an input it used to
        # accept, where the compile fixture proving it lives in the runtime
        # partner rather than in the macro crate.
        function New-RejectionFacts {
            param([bool]$Modified = $true)

            return @(
                New-ReleaseFact -Name macros -Version '0.4.0' -ProcMacroOnly $true `
                    -MacroRuntimePartners @('runtime') -Modified $Modified `
                    -MacroCompileFixtureChanges @(
                        New-CompileFixtureChange -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.rs' -Status 'added'
                        New-CompileFixtureChange -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.stderr' -Status 'added'
                    )
                New-ReleaseFact -Name runtime -Version '0.4.0' -Deps macros `
                    -MacroPublicDeps macros `
                    -ModifiedFiles @('crates/runtime/tests/ui/reject_case.rs')
            )
        }
    }

    It 'blocks a compatible verdict contradicted by a pass to fail fixture' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ macros = 'patch'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('macros', 'runtime') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.rs' `
                            -Baseline 'pass' -Current 'fail'
                    )
            }
        }

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'macroVerdictUnderclassified'
        $ambiguity | Should -Not -BeNullOrEmpty
        $ambiguity.package | Should -Be 'macros'
        $ambiguity.declaredVerdict | Should -Be 'compatible'
        $ambiguity.derivedVerdict | Should -Be 'breaking'
        @($ambiguity.decidingFixtures) |
            Should -Contain 'crates/runtime/tests/ui/reject_case.rs'
    }

    It 'accepts a breaking verdict backed by the same evidence and cascades it' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'targeted'
            tokens = @('macros@breaking')
            classifications = @{ macros = 'breaking'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'breaking' `
                    -ReviewedPackages @('macros', 'runtime') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.rs' `
                            -Baseline 'pass' -Current 'fail'
                    )
            }
        }

        $plan.status | Should -Be 'resolved'
        $macros = $plan.releases | Where-Object folder -eq macros
        $macros.to | Should -Be '0.5.0'
        ($plan.macroContracts | Where-Object package -eq macros).derivedVerdict |
            Should -Be 'breaking'
        # The macro is a public dependency of its runtime facade, so the break
        # has to reach the facade too.
        $runtime = $plan.releases | Where-Object folder -eq runtime
        $runtime.changeType | Should -Be 'breaking'
    }

    It 'lets a compatible verdict stand when the fixture failed before and after' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ macros = 'patch'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('macros', 'runtime') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.rs' `
                            -Baseline 'fail' -Current 'fail'
                    )
            }
        }

        $plan.status | Should -Be 'resolved'
        $macros = $plan.releases | Where-Object folder -eq macros
        $macros.to | Should -Be '0.4.1'
        $macros.changeType | Should -Be 'patch'
        ($plan.macroContracts | Where-Object package -eq macros).derivedVerdict |
            Should -Be 'compatible'
    }

    It 'treats a fail to pass fixture as a non-breaking floor' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ macros = 'patch'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('macros', 'runtime') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.rs' `
                            -Baseline 'fail' -Current 'pass'
                    )
            }
        }

        $plan.status | Should -Be 'blocked'
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'macroVerdictUnderclassified'
        $ambiguity.derivedVerdict | Should -Be 'nonbreaking'
    }

    It 'blocks a contract that leaves a changed fixture unmeasured' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ macros = 'patch'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('macros', 'runtime')
            }
        }

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'macroCompileFixtureUnevidenced'
        $ambiguity | Should -Not -BeNullOrEmpty
        $ambiguity.requiredInput | Should -Be 'macroContracts.macros.compileEvidence'
        @($ambiguity.fixtures) |
            Should -Contain 'crates/runtime/tests/ui/reject_case.rs'
    }

    It 'discharges an expectation file through its compiled sibling' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'targeted'
            tokens = @('macros@breaking')
            classifications = @{ macros = 'breaking'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'breaking' `
                    -ReviewedPackages @('macros', 'runtime') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.stderr' `
                            -Baseline 'pass' -Current 'fail'
                    )
            }
        }

        # One measurement of the fixture answers for both the .rs case and its
        # recorded .stderr expectation.
        $plan.status | Should -Be 'resolved'
        ($plan.macroContracts | Where-Object package -eq macros).derivedVerdict |
            Should -Be 'breaking'
    }

    It 'blocks evidence that does not record a usable measurement' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ macros = 'patch'; runtime = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('macros', 'runtime') `
                    -CompileEvidence @(
                        @{
                            ownerPackage = 'runtime'
                            path = 'crates/runtime/tests/ui/reject_case.rs'
                            baseline = @{ result = 'pass'; revision = 'abc123'; exitCode = 0 }
                            current = @{ result = 'unknown'; revision = ''; exitCode = $null }
                        }
                    )
            }
        }

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        ($plan.ambiguities | Where-Object kind -eq 'macroCompileEvidenceInconclusive') |
            Should -Not -BeNullOrEmpty
    }

    It 'rejects evidence that names no fixture' {
        {
            Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
                mode = 'targeted'
                tokens = @('macros@patch')
                classifications = @{ macros = 'patch'; runtime = 'patch' }
                macroContracts = @{
                    macros = New-MacroContract -Verdict 'compatible' `
                        -ReviewedPackages @('macros', 'runtime') `
                        -CompileEvidence @(@{ path = 'crates/runtime/tests/ui/reject_case.rs' })
                }
            }
        } | Should -Throw '*must name ownerPackage and path*'
    }

    It 'refuses a behavior-fix selection reason for a derived break' {
        {
            Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
                mode = 'changed'
                tokens = @('macros@breaking', 'runtime@patch')
                selectionDecisions = @{
                    macros = New-SelectionDecision -Reason 'behavior-fix'
                    runtime = New-SelectionDecision -Reason 'behavior-fix'
                }
                classifications = @{ macros = 'breaking'; runtime = 'patch' }
                macroContracts = @{
                    macros = New-MacroContract -Verdict 'breaking' `
                        -ReviewedPackages @('macros', 'runtime') `
                        -CompileEvidence @(
                            New-CompileEvidence -OwnerPackage 'runtime' `
                                -Path 'crates/runtime/tests/ui/reject_case.rs' `
                                -Baseline 'pass' -Current 'fail'
                        )
                }
            }
        } | Should -Throw "*conflicts with the 'breaking' macro contract*"
    }

    It 'refuses to decline a proc macro whose fixtures prove a break' {
        {
            Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
                mode = 'changed'
                tokens = @('runtime@patch')
                selectionDecisions = @{
                    macros = New-SelectionDecision -Decision 'decline' -Reason 'test-only'
                    runtime = New-SelectionDecision -Reason 'behavior-fix'
                }
                classifications = @{ runtime = 'patch' }
                macroContracts = @{
                    macros = New-MacroContract -Verdict 'breaking' `
                        -ReviewedPackages @('macros', 'runtime') `
                        -CompileEvidence @(
                            New-CompileEvidence -OwnerPackage 'runtime' `
                                -Path 'crates/runtime/tests/ui/reject_case.rs' `
                                -Baseline 'pass' -Current 'fail'
                        )
                }
            }
        } | Should -Throw '*declines a package whose compile evidence*'
    }

    It 'blocks a declined proc macro that never explains its fixture changes' {
        $plan = Invoke-ReleasePlan -Facts (New-RejectionFacts) -Request @{
            mode = 'changed'
            tokens = @('runtime@patch')
            selectionDecisions = @{
                macros = New-SelectionDecision -Decision 'decline' -Reason 'test-only'
                runtime = New-SelectionDecision -Reason 'behavior-fix'
            }
            classifications = @{ runtime = 'patch' }
        }

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        ($plan.ambiguities | Where-Object kind -eq 'macroContractUnreviewed') |
            Should -Not -BeNullOrEmpty
    }

    It 'reports but does not derive a floor from a published dependency fixture' {
        # The fixture belongs to a published implementation dependency, which
        # carries its own classification; it must not silently reclassify every
        # macro that merely depends on that crate.
        $facts = @(
            New-ReleaseFact -Name helper -Version '0.4.0' `
                -ModifiedFiles @('crates/helper/tests/ui/reject_case.rs')
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps helper `
                -ProcMacroOnly $true -MacroImplementationClosure helper `
                -MacroCompileFixtureChanges @(
                    New-CompileFixtureChange -OwnerPackage 'helper' `
                        -Path 'crates/helper/tests/ui/reject_case.rs' -Status 'added' `
                        -ScopeRole 'implementationClosure' -OwnerPublished $true
                )
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('helper@patch')
            classifications = @{ helper = 'patch'; macros = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('helper', 'macros') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'helper' `
                            -Path 'crates/helper/tests/ui/reject_case.rs' `
                            -Baseline 'pass' -Current 'fail'
                    )
            }
        }

        $plan.status | Should -Be 'resolved'
        ($plan.macroContracts | Where-Object package -eq macros).derivedVerdict |
            Should -Be 'compatible'
    }

    It 'derives a floor from an unpublished helper fixture' {
        # An unpublished helper has no release identity of its own, so its
        # fixtures can only be speaking about the macro that consumes it.
        $facts = @(
            New-ReleaseFact -Name helper -Version '0.4.0' -Published $false `
                -ModifiedFiles @('crates/helper/tests/ui/reject_case.rs')
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps helper `
                -ProcMacroOnly $true -MacroImplementationClosure helper `
                -MacroCompileFixtureChanges @(
                    New-CompileFixtureChange -OwnerPackage 'helper' `
                        -Path 'crates/helper/tests/ui/reject_case.rs' -Status 'added' `
                        -ScopeRole 'implementationClosure' -OwnerPublished $false
                )
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ macros = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('helper', 'macros') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'helper' `
                            -Path 'crates/helper/tests/ui/reject_case.rs' `
                            -Baseline 'pass' -Current 'fail'
                    )
            }
        }

        $plan.status | Should -Be 'blocked'
        ($plan.ambiguities | Where-Object kind -eq 'macroVerdictUnderclassified').derivedVerdict |
            Should -Be 'breaking'
    }

    It 'raises the floor from evidence for a fixture the facts did not flag' {
        $facts = @(
            New-ReleaseFact -Name macros -Version '0.4.0' -ProcMacroOnly $true `
                -MacroRuntimePartners @('runtime')
            New-ReleaseFact -Name runtime -Version '0.4.0' -Deps macros `
                -MacroPublicDeps macros -Modified $false
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros@patch')
            classifications = @{ macros = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -Verdict 'compatible' `
                    -ReviewedPackages @('macros') `
                    -CompileEvidence @(
                        New-CompileEvidence -OwnerPackage 'runtime' `
                            -Path 'crates/runtime/tests/ui/reject_case.rs' `
                            -Baseline 'pass' -Current 'fail'
                    )
            }
        }

        $plan.status | Should -Be 'blocked'
        ($plan.ambiguities | Where-Object kind -eq 'macroVerdictUnderclassified').derivedVerdict |
            Should -Be 'breaking'
    }
}

Describe 'resolve-plan.ps1 breaking selection evidence' {
    It 'blocks a breaking selection whose own objective classification is patch' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name facade) `
            -Request @{
                mode = 'changed'
                tokens = @('facade@breaking')
                selectionDecisions = @{
                    facade = New-SelectionDecision -Reason breaking
                }
                classifications = @{ facade = 'patch' }
            }

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'breakingSelectionUnderclassified'
        $ambiguity.package | Should -Be 'facade'
        $ambiguity.objectiveClassification | Should -Be 'compatible'
    }

    It 'accepts a breaking selection supported by its own classification' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package) `
            -Request @{
                mode = 'changed'
                tokens = @('package@breaking')
                selectionDecisions = @{
                    package = New-SelectionDecision -Reason breaking
                }
                classifications = @{ package = 'breaking' }
            }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].changeType | Should -Be 'breaking'
    }
}

Describe 'resolve-plan.ps1 decline-reason precedence' {
    It 'rejects generated-artifact-only when a Cargo.toml also changed' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name pkg `
                        -ModifiedFiles @('crates/pkg/Cargo.toml', 'crates/pkg/README.md')) `
                -Request @{
                    mode = 'changed'
                    tokens = @()
                    selectionDecisions = @{
                        pkg = New-SelectionDecision -Decision decline -Reason generated-artifact-only
                    }
                    classifications = @{ pkg = 'patch' }
                }
        } | Should -Throw "*only this crate's generated README.md or CHANGELOG.md*"
    }

    It 'accepts generated-artifact-only when only generated files changed' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name pkg `
                    -ModifiedFiles @('crates/pkg/README.md', 'crates/pkg/CHANGELOG.md')) `
            -Request @{
                mode = 'changed'
                tokens = @()
                selectionDecisions = @{
                    pkg = New-SelectionDecision -Decision decline -Reason generated-artifact-only
                }
                classifications = @{ pkg = 'patch' }
            }

        $plan.status | Should -Be 'resolved'
        $plan.selectionDecisions[0].reason | Should -Be 'generated-artifact-only'
    }

    It 'rejects release-metadata-only when only a generated file changed' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name pkg `
                        -ModifiedFiles @('crates/pkg/README.md')) `
                -Request @{
                    mode = 'changed'
                    tokens = @()
                    selectionDecisions = @{
                        pkg = New-SelectionDecision -Decision decline -Reason release-metadata-only
                    }
                    classifications = @{ pkg = 'patch' }
                }
        } | Should -Throw "*use 'generated-artifact-only'*"
    }

    It 'rejects generated-artifact-only for an out-of-package README path' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name pkg `
                        -ModifiedFiles @('crates/other/README.md')) `
                -Request @{
                    mode = 'changed'
                    tokens = @()
                    selectionDecisions = @{
                        pkg = New-SelectionDecision -Decision decline -Reason generated-artifact-only
                    }
                    classifications = @{ pkg = 'patch' }
                }
        } | Should -Throw "*only this crate's generated README.md or CHANGELOG.md*"
    }
}

Describe 'resolve-plan.ps1 review-scope normalization' {
    It 'emits the computed review scope, not a model-supplied superset' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps core `
                -MacroImplementationClosure core -MacroRuntimePartners runtime `
                -ProcMacroOnly $true
            New-ReleaseFact -Name runtime -Modified $false -WorkspaceModified $false
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch' }
            macroContracts = @{
                macros = New-MacroContract -ReviewedPackages @('core', 'macros', 'runtime')
            }
        }

        $plan.status | Should -Be 'resolved'
        $macros = $plan.macroContracts | Where-Object package -eq macros
        # 'runtime' is an unmodified partner: not in the required scope, so the
        # emitted scope drops it even though the model listed it.
        @($macros.reviewed) | Should -Be @('core', 'macros')
    }
}

Describe 'resolve-plan.ps1 own-diff classification floor' {
    It 'blocks a breaking classification when only doc comments changed' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name facade `
                    -RustImplementationChanged $false -DocCommentChanged $true) `
            -Request @{
                mode = 'changed'
                tokens = @('facade@breaking')
                selectionDecisions = @{
                    facade = New-SelectionDecision -Reason authored-doc-fix
                }
                classifications = @{ facade = 'breaking' }
            }

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'ownClassificationUnsupported'
        $ambiguity.package | Should -Be 'facade'
        $ambiguity.requiredInput | Should -Be 'classifications.facade'
    }

    It 'blocks a nonbreaking classification when only doc comments changed' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name facade `
                    -RustImplementationChanged $false -DocCommentChanged $true) `
            -Request @{
                mode = 'changed'
                tokens = @('facade@nonbreaking')
                selectionDecisions = @{
                    facade = New-SelectionDecision -Reason authored-doc-fix
                }
                classifications = @{ facade = 'nonbreaking' }
            }

        $plan.status | Should -Be 'blocked'
        @($plan.ambiguities | Where-Object kind -eq 'ownClassificationUnsupported') |
            Should -Not -BeNullOrEmpty
    }

    It 'allows an authored-doc-fix patch when only doc comments changed' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name facade `
                    -RustImplementationChanged $false -DocCommentChanged $true) `
            -Request @{
                mode = 'changed'
                tokens = @('facade')
                selectionDecisions = @{
                    facade = New-SelectionDecision -Reason authored-doc-fix
                }
                classifications = @{ facade = 'patch' }
            }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].changeType | Should -Be 'patch'
    }

    It 'rejects declining a doc-only authored change as internal-only' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name facade `
                        -RustImplementationChanged $false -DocCommentChanged $true) `
                -Request @{
                    mode = 'changed'
                    tokens = @()
                    selectionDecisions = @{
                        facade = New-SelectionDecision -Decision decline -Reason internal-only
                    }
                    classifications = @{ facade = 'patch' }
                }
        } | Should -Throw "*must be accepted as 'authored-doc-fix'*"
    }

    It 'rejects pairing authored-doc-fix with a runtime-manifest change' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name pkg `
                        -RustImplementationChanged $false -DocCommentChanged $true `
                        -ModifiedFiles @('crates/pkg/src/lib.rs', 'crates/pkg/Cargo.toml') `
                        -ManifestDependencyScopes @('normal')) `
                -Request @{
                    mode = 'changed'
                    tokens = @('pkg')
                    selectionDecisions = @{
                        pkg = New-SelectionDecision -Reason authored-doc-fix
                    }
                    classifications = @{ pkg = 'patch' }
                }
        } | Should -Throw "*use 'runtime-manifest-change'*"
    }

    It 'allows internal-only for a non-doc comment or whitespace source edit' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name facade `
                    -RustImplementationChanged $false -DocCommentChanged $false) `
            -Request @{
                mode = 'changed'
                tokens = @()
                selectionDecisions = @{
                    facade = New-SelectionDecision -Decision decline -Reason internal-only
                }
                classifications = @{ facade = 'patch' }
            }

        $plan.status | Should -Be 'resolved'
        @($plan.releases).Count | Should -Be 0
    }

    It 'does not force authored-doc-fix when the source change is real implementation' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name facade -RustImplementationChanged $true) `
            -Request @{
                mode = 'changed'
                tokens = @()
                selectionDecisions = @{
                    facade = New-SelectionDecision -Decision decline -Reason internal-only
                }
                classifications = @{ facade = 'patch' }
            }

        $plan.status | Should -Be 'resolved'
        @($plan.releases).Count | Should -Be 0
    }

    It 'allows a breaking classification when Rust implementation changed' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package -RustImplementationChanged $true) `
            -Request @{
                mode = 'changed'
                tokens = @('package@breaking')
                selectionDecisions = @{
                    package = New-SelectionDecision -Reason breaking
                }
                classifications = @{ package = 'breaking' }
            }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].changeType | Should -Be 'breaking'
    }

    It 'exempts a package whose breaking is forced by an exposed external dep' {
        $facts = @(
            New-ReleaseFact -Name macro_impl -Version '0.2.0' `
                -RustImplementationChanged $false `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'changed'
            tokens = @('macro_impl@breaking')
            selectionDecisions = @{
                macro_impl = New-SelectionDecision -Decision accept -Reason breaking
            }
            classifications = @{ macro_impl = 'breaking' }
        }

        $plan.status | Should -Be 'resolved'
        @($plan.ambiguities | Where-Object kind -eq 'ownClassificationUnsupported') |
            Should -BeNullOrEmpty
        $plan.releases[0].changeType | Should -Be 'breaking'
    }

    It 'does not constrain the own classification of a first release' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name fresh -Version '0.1.0' `
                    -EverReleased $false -RustImplementationChanged $false `
                    -ModifiedFiles @('crates/fresh/src/lib.rs')) `
            -Request @{
                mode = 'changed'
                tokens = @('fresh')
                selectionDecisions = @{
                    fresh = New-SelectionDecision -Reason first-release
                }
                classifications = @{}
            }

        @($plan.ambiguities | Where-Object kind -eq 'ownClassificationUnsupported') |
            Should -BeNullOrEmpty
    }
}

Describe 'resolve-plan.ps1 behavior-fix evidence' {
    BeforeAll {
        function Invoke-BehaviorFixPlan {
            param(
                [AllowNull()][AllowEmptyCollection()][object[]]$RegressionEvidence,
                [switch]$OmitRegressionEvidence,
                [string]$Reason = 'behavior-fix'
            )

            $decision = if ($OmitRegressionEvidence) {
                New-SelectionDecision -Reason $Reason -RegressionEvidence @()
            } elseif ($PSBoundParameters.ContainsKey('RegressionEvidence')) {
                New-SelectionDecision -Reason $Reason -RegressionEvidence $RegressionEvidence
            } else {
                New-SelectionDecision -Reason $Reason
            }

            return Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package -Version '1.2.3') `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{ package = $decision }
                    classifications = @{ package = 'patch' }
                }
        }
    }

    It 'releases a behavior fix whose probe failed at the baseline and now passes' {
        $plan = Invoke-BehaviorFixPlan

        $plan.status | Should -Be 'resolved'
        @($plan.releases).Count | Should -Be 1
        $plan.releases[0].folder | Should -Be 'package'
        $plan.selectionDecisions[0].regressionEvidence[0].outcome | Should -Be 'fail->pass'
    }

    It 'blocks a behavior fix that records no probe at all' {
        $plan = Invoke-BehaviorFixPlan -OmitRegressionEvidence

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'behaviorFixUndemonstrated'
        $ambiguity | Should -Not -BeNullOrEmpty
        $ambiguity.package | Should -Be 'package'
        $ambiguity.requiredInput |
            Should -Be 'selectionDecisions.package.regressionEvidence'
    }

    BeforeDiscovery {
        $unchangedCases = @(
            @{ Name = 'preserved behavior'; Baseline = 'pass'; Current = 'pass' }
            @{ Name = 'still broken behavior'; Baseline = 'fail'; Current = 'fail' }
            @{ Name = 'newly broken behavior'; Baseline = 'pass'; Current = 'fail' }
        )
        $kindCases = @(
            @{ Kind = 'consumer-runtime' }
            @{ Kind = 'consumer-compile' }
            @{ Kind = 'packaged-artifact' }
        )
    }

    It 'blocks a behavior fix whose probe shows <Name>' -ForEach $unchangedCases {
        $plan = Invoke-BehaviorFixPlan -RegressionEvidence @(
            New-RegressionEvidence -Baseline $Baseline -Current $Current
        )

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'behaviorFixUndemonstrated'
        $ambiguity | Should -Not -BeNullOrEmpty
        $ambiguity.probes[0].outcome | Should -Be "$Baseline->$Current"
    }

    It 'accepts a demonstrated fix measured by a <Kind> probe' -ForEach $kindCases {
        $plan = Invoke-BehaviorFixPlan -RegressionEvidence @(
            New-RegressionEvidence -Kind $Kind
        )

        $plan.status | Should -Be 'resolved'
        $plan.selectionDecisions[0].regressionEvidence[0].kind | Should -Be $Kind
    }

    It 'keeps a demonstrated probe alongside probes that did not move' {
        $plan = Invoke-BehaviorFixPlan -RegressionEvidence @(
            New-RegressionEvidence -Probe 'cargo test --test unaffected' `
                -Baseline 'pass' -Current 'pass'
            New-RegressionEvidence -Kind 'packaged-artifact' `
                -Probe 'cargo package --list'
        )

        $plan.status | Should -Be 'resolved'
        @($plan.selectionDecisions[0].regressionEvidence).Count | Should -Be 2
    }

    It 'blocks a probe whose baseline was never measured' {
        $plan = Invoke-BehaviorFixPlan -RegressionEvidence @(
            New-RegressionEvidence -BaselineRevision ''
        )

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'behaviorEvidenceInconclusive'
        $ambiguity | Should -Not -BeNullOrEmpty
        $ambiguity.issues -join ' ' | Should -BeLike '*baseline pass/fail result*'
    }

    It 'blocks a probe with no exit code' {
        $plan = Invoke-BehaviorFixPlan -RegressionEvidence @(
            New-RegressionEvidence -CurrentExitCode $null
        )

        $plan.status | Should -Be 'blocked'
        ($plan.ambiguities | Where-Object kind -eq 'behaviorEvidenceInconclusive') |
            Should -Not -BeNullOrEmpty
    }

    It 'blocks a probe whose exit code contradicts its result' {
        $plan = Invoke-BehaviorFixPlan -RegressionEvidence @(
            New-RegressionEvidence -CurrentExitCode 101
        )

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        $ambiguity = $plan.ambiguities |
            Where-Object kind -eq 'behaviorEvidenceInconclusive'
        $ambiguity.issues -join ' ' | Should -BeLike "*result of 'pass' with exit code 101*"
        ($plan.ambiguities | Where-Object kind -eq 'behaviorFixUndemonstrated') |
            Should -Not -BeNullOrEmpty
    }

    It 'blocks a probe that measured one revision twice' {
        $plan = Invoke-BehaviorFixPlan -RegressionEvidence @(
            New-RegressionEvidence -BaselineRevision 'worktree'
        )

        $plan.status | Should -Be 'blocked'
        ($plan.ambiguities | Where-Object kind -eq 'behaviorEvidenceInconclusive').issues -join ' ' |
            Should -BeLike '*on both sides*'
    }

    It 'rejects a probe that names no command' {
        { Invoke-BehaviorFixPlan -RegressionEvidence @(New-RegressionEvidence -Probe ' ') } |
            Should -Throw '*must name the probe it exercised*'
    }

    It 'rejects a probe measured by an unrecognized kind' {
        { Invoke-BehaviorFixPlan -RegressionEvidence @(New-RegressionEvidence -Kind 'vibes') } |
            Should -Throw '*must use kind consumer-runtime, consumer-compile, packaged-artifact*'
    }

    It 'rejects regression evidence written as prose' {
        { Invoke-BehaviorFixPlan -RegressionEvidence @('The bug is fixed.') } |
            Should -Throw '*must be an object with kind, probe, baseline, and current*'
    }

    It 'leaves other accepted reasons unaffected' {
        $plan = Invoke-BehaviorFixPlan -Reason 'nonbreaking-api' -OmitRegressionEvidence

        $plan.status | Should -Be 'resolved'
        @($plan.releases).Count | Should -Be 1
    }

    It 'leaves declined packages unaffected' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name package -Version '1.2.3'
                New-ReleaseFact -Name other -Version '1.0.0'
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('package')
                selectionDecisions = @{
                    package = New-SelectionDecision -Reason 'nonbreaking-api'
                    other = New-SelectionDecision -Decision 'decline' -Reason 'internal-only'
                }
                classifications = @{ package = 'patch' }
            }

        $plan.status | Should -Be 'resolved'
        @($plan.releases).Count | Should -Be 1
    }

    It 'blocks every undemonstrated behavior fix in the same plan' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name first -Version '1.2.3'
                New-ReleaseFact -Name second -Version '1.2.3'
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('first', 'second')
                selectionDecisions = @{
                    first = New-SelectionDecision -RegressionEvidence @(
                        New-RegressionEvidence -Baseline 'pass' -Current 'pass'
                    )
                    second = New-SelectionDecision -RegressionEvidence @()
                }
                classifications = @{ first = 'patch'; second = 'patch' }
            }

        $plan.status | Should -Be 'blocked'
        @($plan.releases).Count | Should -Be 0
        @($plan.ambiguities | Where-Object kind -eq 'behaviorFixUndemonstrated').Count |
            Should -Be 2
    }
}

Describe 'resolve-plan.ps1 pins and validation' {
    It 'honors a pin that already satisfies a breaking cascade floor' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name consumer -Deps core -ExposedDeps core
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking', 'consumer@5.0.0')
            classifications = @{ core = 'patch'; consumer = 'patch' }
        }

        ($plan.releases | Where-Object folder -eq consumer).to | Should -Be '5.0.0'
    }

    It 'rejects a pin below the required cascade target' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name consumer -Deps core -ExposedDeps core
        )

        {
            Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'targeted'
                tokens = @('core@breaking', 'consumer@1.1.0')
                classifications = @{ core = 'patch'; consumer = 'patch' }
            }
        } | Should -Throw '*below the required*'
    }

    It 'keeps a conflicting pin under force and emits a warning' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name consumer -Deps core -ExposedDeps core
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking', 'consumer@1.1.0')
            classifications = @{ core = 'patch'; consumer = 'patch' }
            force = $true
        }

        $consumer = $plan.releases | Where-Object folder -eq consumer
        $consumer.to | Should -Be '1.1.0'
        $consumer.changeType | Should -Be 'breaking'
        @($plan.warnings).Count | Should -Be 1
    }

    It 'rejects pins that are not strictly greater than the current version' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package -Version '1.2.3') `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package@1.2.3')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*must be strictly greater*'
    }

    It 'rejects build-only pins because build metadata has equal precedence' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package -Version '1.2.3+old') `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package@1.2.3+new')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*must be strictly greater*'
    }

    It 'accepts a greater pin and preserves build metadata' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package -Version '1.2.3-alpha.1') `
            -Request @{
                mode = 'targeted'
                tokens = @('package@1.2.4+build.7')
                classifications = @{ package = 'patch' }
            }

        $plan.releases[0].to | Should -Be '1.2.4+build.7'
    }

    It 'matches hyphenated Cargo names from normalized tokens' {
        $fact = New-ReleaseFact -Name package_name
        $fact.name = 'package-name'
        $plan = Invoke-ReleasePlan -Facts @($fact) -Request @{
            mode = 'targeted'
            tokens = @('package-name@patch')
            classifications = @{ package_name = 'patch' }
        }

        $plan.releases[0].folder | Should -Be 'package_name'
    }

    It 'fails rather than guessing a missing ordinary-library classification' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{ mode = 'targeted'; tokens = @('package'); classifications = @{} }
        } | Should -Throw '*Missing objective classification*'
    }

    It 'rejects duplicate package tokens' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package@patch', 'package@breaking')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*appears more than once*'
    }

    It 'rejects unknown and unpublished packages' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('missing')
                    classifications = @{}
                }
        } | Should -Throw '*matched 0 workspace packages*'

        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package -Published $false) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package')
                    classifications = @{}
                }
        } | Should -Throw '*is not publishable*'
    }

    It 'rejects malformed modes, change types, and empty requests' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'invalid'
                    tokens = @('package')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*Unknown release mode*'

        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package@major')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*Invalid SemVer version*'

        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'targeted'
                    tokens = @()
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*requires at least one accepted package token*'
    }

    It 'rejects stale facts schemas and missing macro fact fields' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package')
                    classifications = @{ package = 'patch' }
                } `
                -SchemaVersion 3
        } | Should -Throw '*unsupported schema*'

        $fact = New-ReleaseFact -Name package
        $fact.Remove('macroPublicDeps')
        {
            Invoke-ReleasePlan `
                -Facts @($fact) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw "*missing 'macroPublicDeps'*"

        $fact = New-ReleaseFact -Name package
        $fact.Remove('modifiedFiles')
        {
            Invoke-ReleasePlan `
                -Facts @($fact) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw "*missing 'modifiedFiles'*"

        $fact = New-ReleaseFact -Name package
        $fact.Remove('manifestDependencyScopes')
        {
            Invoke-ReleasePlan `
                -Facts @($fact) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw "*missing 'manifestDependencyScopes'*"

        $fact = New-ReleaseFact -Name package
        $fact.Remove('manifestOtherChanged')
        {
            Invoke-ReleasePlan `
                -Facts @($fact) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package')
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw "*missing 'manifestOtherChanged'*"
    }

    It 'treats a JSON null modifiedFiles field as an empty file list' {
        $fact = New-ReleaseFact -Name package
        $fact.modifiedFiles = $null
        $fact.modifiedFileCount = 0

        $plan = Invoke-ReleasePlan `
            -Facts @($fact) `
            -Request @{
                mode = 'changed'
                tokens = @()
                selectionDecisions = @{
                    package = New-SelectionDecision -Decision decline -Reason internal-only
                }
                classifications = @{}
            }

        $plan.status | Should -Be 'resolved'
        @($plan.releases).Count | Should -Be 0
    }

    It 'rejects malformed and contradictory macro contracts clearly' {
        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact -Name macros -ProcMacroOnly $true
                ) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('macros@patch')
                    classifications = @{}
                    macroContracts = @{
                        macros = @{ verdict = 'compatible' }
                    }
                }
        } | Should -Throw '*must include reviewedPackages, channels, and evidence*'

        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact -Name macros -ProcMacroOnly $true
                ) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('macros@breaking')
                    classifications = @{}
                    macroContracts = @{
                        macros = New-MacroContract
                    }
                }
        } | Should -Throw '*conflicts with its*contract verdict*'
    }

    It 'preserves changed and all mode labels after package selection' {
        foreach ($mode in @('changed', 'all')) {
            $plan = Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = $mode
                    tokens = @('package')
                    selectionDecisions = @{
                        package = New-SelectionDecision
                    }
                    classifications = @{ package = 'patch' }
                }
            $plan.mode | Should -Be $mode
        }
    }

    It 'requires complete selection decisions in changed mode' {
        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact -Name accepted
                    New-ReleaseFact -Name omitted
                ) `
                -Request @{
                    mode = 'changed'
                    tokens = @('accepted')
                    selectionDecisions = @{
                        accepted = New-SelectionDecision
                    }
                    classifications = @{
                        accepted = 'patch'
                        omitted = 'patch'
                    }
                }
        } | Should -Throw '*missing candidate packages: omitted*'
    }

    It 'requires tokens to exactly match accepted changed-mode decisions' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{
                        package = New-SelectionDecision `
                            -Decision decline `
                            -Reason test-only
                    }
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*conflicts with its decline selection decision*'

        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name accepted) `
                -Request @{
                    mode = 'changed'
                    tokens = @()
                    selectionDecisions = @{
                        accepted = New-SelectionDecision
                    }
                    classifications = @{ accepted = 'patch' }
                }
        } | Should -Throw "*Accepted selection decision 'accepted' is missing*"
    }

    It 'rejects a dev-only manifest change as a runtime release seed' {
        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact `
                        -Name package `
                        -ModifiedFiles 'crates/package/Cargo.toml' `
                        -ManifestDependencyScopes dev
                ) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{
                        package = New-SelectionDecision `
                            -Reason runtime-manifest-change
                    }
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw "*requires a changed normal/build dependency or package feature*"
    }

    It 'rejects declining a runtime manifest change' {
        foreach ($scope in @('normal', 'build', 'features')) {
            {
                Invoke-ReleasePlan `
                    -Facts @(
                        New-ReleaseFact `
                            -Name package `
                            -ModifiedFiles 'crates/package/Cargo.toml' `
                            -ManifestDependencyScopes $scope
                    ) `
                    -Request @{
                        mode = 'changed'
                        tokens = @()
                        selectionDecisions = @{
                            package = New-SelectionDecision `
                                -Decision decline `
                                -Reason test-only
                        }
                        classifications = @{ package = 'patch' }
                    }
            } | Should -Throw '*cannot decline a changed normal/build dependency or package feature*'
        }
    }

    It 'rejects relabeling a dev-only manifest change as another accepted reason' {
        foreach ($reason in @(
                'breaking',
                'nonbreaking-api',
                'behavior-fix',
                'authored-doc-fix'
            )) {
            {
                Invoke-ReleasePlan `
                    -Facts @(
                        New-ReleaseFact `
                            -Name package `
                            -ModifiedFiles @(
                                'crates/package/Cargo.toml',
                                'crates/package/README.md'
                            ) `
                            -ManifestDependencyScopes dev
                    ) `
                    -Request @{
                        mode = 'changed'
                        tokens = @('package')
                        selectionDecisions = @{
                            package = New-SelectionDecision -Reason $reason
                        }
                        classifications = @{ package = 'patch' }
                    }
            } | Should -Throw '*cannot accept a dev-dependency-only manifest change*'
        }
    }

    It 'allows an evidenced non-dependency manifest change alongside a dev dependency edit' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact `
                    -Name package `
                    -ModifiedFiles 'crates/package/Cargo.toml' `
                    -ManifestDependencyScopes dev `
                    -ManifestOtherChanged $true
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('package')
                selectionDecisions = @{
                    package = New-SelectionDecision -Reason behavior-fix
                }
                classifications = @{ package = 'patch' }
            }

        $plan.releases[0].folder | Should -Be 'package'
    }

    It 'allows benchmark-only manifest and authored-file changes to decline' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact `
                    -Name package `
                    -ModifiedFiles @(
                        'crates/package/Cargo.toml',
                        'crates/package/benches/throughput.rs'
                    ) `
                    -ManifestOtherChanged $true
            ) `
            -Request @{
                mode = 'changed'
                tokens = @()
                selectionDecisions = @{
                    package = New-SelectionDecision `
                        -Decision decline `
                        -Reason benchmark-only
                }
                classifications = @{ package = 'patch' }
            }

        $plan.releases.Count | Should -Be 0
    }

    It 'requires the canonical reason for a pure dev dependency manifest edit' {
        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact `
                        -Name package `
                        -ModifiedFiles 'crates/package/Cargo.toml' `
                        -ManifestDependencyScopes dev
                ) `
                -Request @{
                    mode = 'changed'
                    tokens = @()
                    selectionDecisions = @{
                        package = New-SelectionDecision `
                            -Decision decline `
                            -Reason release-metadata-only
                    }
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw "*must classify a pure dev dependency manifest edit as 'dev-dependency-only'*"
    }

    It 'accepts runtime dependency changes and dev-only declines' {
        $runtimePlan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact `
                    -Name runtime `
                    -ModifiedFiles 'crates/runtime/Cargo.toml' `
                    -ManifestDependencyScopes normal
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('runtime')
                selectionDecisions = @{
                    runtime = New-SelectionDecision `
                        -Reason runtime-manifest-change
                }
                classifications = @{ runtime = 'patch' }
            }
        $runtimePlan.releases[0].folder | Should -Be 'runtime'

        $featurePlan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact `
                    -Name features `
                    -ModifiedFiles 'crates/features/Cargo.toml' `
                    -ManifestDependencyScopes features
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('features')
                selectionDecisions = @{
                    features = New-SelectionDecision `
                        -Reason runtime-manifest-change
                }
                classifications = @{ features = 'patch' }
            }
        $featurePlan.releases[0].folder | Should -Be 'features'

        $devPlan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact `
                    -Name devonly `
                    -ModifiedFiles 'crates/devonly/Cargo.toml' `
                    -ManifestDependencyScopes dev
            ) `
            -Request @{
                mode = 'changed'
                tokens = @()
                selectionDecisions = @{
                    devonly = New-SelectionDecision `
                        -Decision decline `
                        -Reason dev-dependency-only
                }
                classifications = @{ devonly = 'patch' }
            }
        $devPlan.releases.Count | Should -Be 0
    }

    It 'rejects dev-dependency-only when other authored files changed' {
        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact `
                        -Name package `
                        -ModifiedFiles @(
                            'crates/package/Cargo.toml',
                            'crates/package/src/lib.rs'
                        ) `
                        -ManifestDependencyScopes dev
                ) `
                -Request @{
                    mode = 'changed'
                    tokens = @()
                    selectionDecisions = @{
                        package = New-SelectionDecision `
                            -Decision decline `
                            -Reason dev-dependency-only
                    }
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*cannot ignore changed source*'
    }

    It 'emits normalized selection decisions' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name accepted
                New-ReleaseFact -Name declined `
                    -ModifiedFiles @('crates/declined/README.md')
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('accepted')
                selectionDecisions = @{
                    accepted = New-SelectionDecision
                    declined = New-SelectionDecision `
                        -Decision decline `
                        -Reason generated-artifact-only
                }
                classifications = @{
                    accepted = 'patch'
                    declined = 'patch'
                }
            }

        @($plan.selectionDecisions.package) |
            Should -Be @('accepted', 'declined')
        $plan.selectionDecisions[1].reason |
            Should -Be 'generated-artifact-only'
    }

    It 'resolves a complete all-declined request as an empty plan' {
        $plan = Invoke-ReleasePlan `
            -Facts @(New-ReleaseFact -Name package) `
            -Request @{
                mode = 'changed'
                tokens = @()
                selectionDecisions = @{
                    package = New-SelectionDecision `
                        -Decision decline `
                        -Reason test-only
                }
                classifications = @{ package = 'patch' }
            }

        $plan.status | Should -Be 'resolved'
        $plan.releases.Count | Should -Be 0
        $plan.selectionDecisions[0].decision | Should -Be 'decline'
    }

    It 'rejects extra or aliased selection decision keys' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{
                        package = New-SelectionDecision
                        package_alias = New-SelectionDecision
                    }
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*unknown or non-candidate packages: package_alias*'

        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package-name) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package-name')
                    selectionDecisions = @{
                        package_name = New-SelectionDecision
                    }
                    classifications = @{ package_name = 'patch' }
                }
        } | Should -Throw '*Use canonical folder identifiers*'
    }

    It 'supports an explicit unchanged release only in all mode' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name package -Modified $false
            ) `
            -Request @{
                mode = 'all'
                tokens = @('package')
                selectionDecisions = @{
                    package = New-SelectionDecision `
                        -Reason explicit-release
                }
                classifications = @{ package = 'patch' }
            }

        @($plan.releases.folder) | Should -Be @('package')

        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{
                        package = New-SelectionDecision `
                            -Reason explicit-release
                    }
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw "*only valid for an unchanged package in all mode*"
    }

    It 'rejects a first release justified only by tests' {
        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact -Name package -EverReleased $false `
                        -ModifiedFiles @('crates/package/tests/behavior.rs')
                ) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{
                        package = New-SelectionDecision -Reason first-release
                    }
                    classifications = @{}
                }
        } | Should -Throw "*requires a changed packaged file outside tests*"
    }

    It 'accepts a first release with changed packaged source' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name package -EverReleased $false `
                    -ModifiedFiles @('crates/package/src/lib.rs')
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('package')
                selectionDecisions = @{
                    package = New-SelectionDecision -Reason first-release
                }
                classifications = @{}
            }

        $plan.releases[0].to | Should -Be '1.0.0'
    }

    It 'requires every accepted never-released package to use first-release' {
        {
            Invoke-ReleasePlan `
                -Facts @(
                    New-ReleaseFact -Name package -EverReleased $false `
                        -ModifiedFiles @('crates/package/tests/behavior.rs')
                ) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{
                        package = New-SelectionDecision -Reason behavior-fix
                    }
                    classifications = @{}
                }
        } | Should -Throw "*must use selection reason 'first-release'*"
    }

    It 'rejects non-packaged or generated first-release evidence' {
        foreach ($path in @(
                'crates/package/logo.png',
                'crates/package/Cargo.toml',
                'crates/package/README.md'
            )) {
            {
                Invoke-ReleasePlan `
                    -Facts @(
                        New-ReleaseFact -Name package -EverReleased $false `
                            -ModifiedFiles @($path)
                    ) `
                    -Request @{
                        mode = 'changed'
                        tokens = @('package')
                        selectionDecisions = @{
                            package = New-SelectionDecision -Reason first-release
                        }
                        classifications = @{}
                    }
            } | Should -Throw "*requires a changed packaged file outside tests*"
        }
    }

    It 'matches first-release paths ordinally instead of as wildcard patterns' {
        $plan = Invoke-ReleasePlan `
            -Facts @(
                New-ReleaseFact -Name '[package]' -EverReleased $false `
                    -ModifiedFiles @('crates/[package]/src/lib.rs')
            ) `
            -Request @{
                mode = 'changed'
                tokens = @('[package]')
                selectionDecisions = @{
                    '[package]' = New-SelectionDecision -Reason first-release
                }
                classifications = @{}
            }

        $plan.releases[0].folder | Should -Be '[package]'
    }

    It 'rejects request-owned manual review flags' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = 'targeted'
                    tokens = @('package')
                    classifications = @{
                        package = @{
                            changeType = 'patch'
                            manualReview = $true
                        }
                    }
                }
        } | Should -Throw "*manualReview for 'package' is resolver-owned*"
    }

    It 'rejects changed-mode tokens for non-candidates' {
        {
            Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package -Modified $false) `
                -Request @{
                    mode = 'changed'
                    tokens = @('package')
                    selectionDecisions = @{}
                    classifications = @{ package = 'patch' }
                }
        } | Should -Throw '*is not a candidate in changed mode*'
    }

    Describe 'resolve-plan.ps1 generated exposure matrix' {
        BeforeDiscovery {
            $versions = @('1.2.3', '0.4.2', '0.0.5')
            $changes = @('patch', 'nonbreaking', 'breaking')
            $exposureCases = foreach ($dependencyVersion in $versions) {
                foreach ($change in $changes) {
                    foreach ($consumerVersion in $versions) {
                        foreach ($exposed in @($false, $true)) {
                            @{
                                Name = "$dependencyVersion $change -> $consumerVersion exposed=$exposed"
                                DependencyVersion = $dependencyVersion
                                Change = $change
                                ConsumerVersion = $consumerVersion
                                Exposed = $exposed
                            }
                        }
                    }
                }
            }
        }

        It '<Name>' -ForEach $exposureCases {
            $facts = @(
                New-ReleaseFact -Name dependency -Version $DependencyVersion
                New-ReleaseFact `
                    -Name consumer `
                    -Version $ConsumerVersion `
                    -Deps dependency `
                    -ExposedDeps $(if ($Exposed) { @('dependency') } else { @() })
            )
            $plan = Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'targeted'
                tokens = @("dependency@$Change")
                classifications = @{ dependency = 'patch'; consumer = 'patch' }
            }

            $internalChange = if ($Change -eq 'nonbreaking') {
                'non-breaking'
            } else {
                $Change
            }
            $dependencyBreaks = Test-IsBreakingChange `
                -oldVersion $DependencyVersion `
                -ChangeType $internalChange
            $expectedConsumerChange = if ($Exposed -and $dependencyBreaks) {
                'breaking'
            } else {
                'patch'
            }
            $consumer = $plan.releases | Where-Object folder -eq consumer
            $consumer.changeType | Should -Be $expectedConsumerChange
            $consumer.to | Should -Be (
                Get-NextVersion `
                    -currentVersion $ConsumerVersion `
                    -ChangeType $expectedConsumerChange
            )
            $consumer.cascadeReasons[0].breaking |
                Should -Be ($Exposed -and $dependencyBreaks)
        }
    }

    Describe 'resolve-plan.ps1 generated lower-bound matrix' {
        BeforeDiscovery {
            $versions = @('1.2.3', '0.4.2', '0.0.5')
            $objectives = @('patch', 'nonbreaking', 'breaking')
            $requests = @('', 'patch', 'nonbreaking', 'breaking')
            $lowerBoundCases = foreach ($version in $versions) {
                foreach ($objective in $objectives) {
                    foreach ($request in $requests) {
                        @{
                            Name = "$version objective=$objective request=$request"
                            Version = $version
                            Objective = $objective
                            Request = $request
                        }
                    }
                }
            }
        }

        It '<Name>' -ForEach $lowerBoundCases {
            $rank = @{ patch = 1; nonbreaking = 2; breaking = 3 }
            $expectedChange = if (
                [string]::IsNullOrEmpty($Request) -or
                $rank[$Objective] -ge $rank[$Request]
            ) {
                $Objective
            } else {
                $Request
            }
            $token = if ([string]::IsNullOrEmpty($Request)) {
                'package'
            } else {
                "package@$Request"
            }
            $plan = Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package -Version $Version) `
                -Request @{
                    mode = 'targeted'
                    tokens = @($token)
                    classifications = @{ package = $Objective }
                }

            $internalChange = if ($expectedChange -eq 'nonbreaking') {
                'non-breaking'
            } else {
                $expectedChange
            }
            $plan.releases[0].changeType | Should -Be $expectedChange
            $plan.releases[0].to | Should -Be (
                Get-NextVersion `
                    -currentVersion $Version `
                    -ChangeType $internalChange
            )
        }
    }

    Describe 'resolve-plan.ps1 generated fixed-point matrix' {
        BeforeDiscovery {
            $versions = @('1.2.3', '0.4.2', '0.0.5')
            $changes = @('patch', 'nonbreaking', 'breaking')
            $fixedPointCases = foreach ($dependencyVersion in $versions) {
                foreach ($change in $changes) {
                    foreach ($firstExposed in @($false, $true)) {
                        foreach ($secondExposed in @($false, $true)) {
                            @{
                                Name = "$dependencyVersion $change edges=$firstExposed/$secondExposed"
                                DependencyVersion = $dependencyVersion
                                Change = $change
                                FirstExposed = $firstExposed
                                SecondExposed = $secondExposed
                            }
                        }
                    }
                }
            }
        }

        It '<Name>' -ForEach $fixedPointCases {
            $facts = @(
                New-ReleaseFact -Name bottom -Version $DependencyVersion
                New-ReleaseFact `
                    -Name middle `
                    -Deps bottom `
                    -ExposedDeps $(if ($FirstExposed) { @('bottom') } else { @() })
                New-ReleaseFact `
                    -Name top `
                    -Deps middle `
                    -ExposedDeps $(if ($SecondExposed) { @('middle') } else { @() })
            )
            $plan = Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'targeted'
                tokens = @("bottom@$Change")
                classifications = @{ bottom = 'patch'; middle = 'patch'; top = 'patch' }
            }

            $internalChange = if ($Change -eq 'nonbreaking') {
                'non-breaking'
            } else {
                $Change
            }
            $bottomBreaks = Test-IsBreakingChange `
                -oldVersion $DependencyVersion `
                -ChangeType $internalChange
            $middleBreaking = $bottomBreaks -and $FirstExposed
            $topBreaking = $middleBreaking -and $SecondExposed
            ($plan.releases | Where-Object folder -eq middle).changeType |
                Should -Be $(if ($middleBreaking) { 'breaking' } else { 'patch' })
            ($plan.releases | Where-Object folder -eq top).changeType |
                Should -Be $(if ($topBreaking) { 'breaking' } else { 'patch' })
        }
    }

    Describe 'resolve-plan.ps1 generated diamond matrix' {
        BeforeDiscovery {
            $versions = @('1.2.3', '0.4.2', '0.0.5')
            $changes = @('patch', 'nonbreaking', 'breaking')
            $diamondCases = foreach ($dependencyVersion in $versions) {
                foreach ($change in $changes) {
                    foreach ($mask in 0..15) {
                        @{
                            Name = "$dependencyVersion $change diamond-mask=$mask"
                            DependencyVersion = $dependencyVersion
                            Change = $change
                            RootToLeft = [bool]($mask -band 1)
                            RootToRight = [bool]($mask -band 2)
                            LeftToTop = [bool]($mask -band 4)
                            RightToTop = [bool]($mask -band 8)
                        }
                    }
                }
            }
        }

        It '<Name>' -ForEach $diamondCases {
            $facts = @(
                New-ReleaseFact -Name root -Version $DependencyVersion
                New-ReleaseFact `
                    -Name left `
                    -Deps root `
                    -ExposedDeps $(if ($RootToLeft) { @('root') } else { @() })
                New-ReleaseFact `
                    -Name right `
                    -Deps root `
                    -ExposedDeps $(if ($RootToRight) { @('root') } else { @() })
                New-ReleaseFact `
                    -Name top `
                    -Deps @('left', 'right') `
                    -ExposedDeps @(
                        if ($LeftToTop) { 'left' }
                        if ($RightToTop) { 'right' }
                    )
            )
            $plan = Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'targeted'
                tokens = @("root@$Change")
                classifications = @{
                    root = 'patch'
                    left = 'patch'
                    right = 'patch'
                    top = 'patch'
                }
            }

            $internalChange = if ($Change -eq 'nonbreaking') {
                'non-breaking'
            } else {
                $Change
            }
            $rootBreaks = Test-IsBreakingChange `
                -oldVersion $DependencyVersion `
                -ChangeType $internalChange
            $leftBreaks = $rootBreaks -and $RootToLeft
            $rightBreaks = $rootBreaks -and $RootToRight
            $topBreaks =
                ($leftBreaks -and $LeftToTop) -or
                ($rightBreaks -and $RightToTop)

            ($plan.releases | Where-Object folder -eq left).changeType |
                Should -Be $(if ($leftBreaks) { 'breaking' } else { 'patch' })
            ($plan.releases | Where-Object folder -eq right).changeType |
                Should -Be $(if ($rightBreaks) { 'breaking' } else { 'patch' })
            $top = $plan.releases | Where-Object folder -eq top
            $top.changeType |
                Should -Be $(if ($topBreaks) { 'breaking' } else { 'patch' })
            @($top.cascadeReasons).Count | Should -Be 2
            @($top.cascadeReasons.target) | Should -Be @('left', 'right')
        }
    }

    It 'rejects dependency cycles in the supplied fact graph' {
        $facts = @(
            New-ReleaseFact -Name left -Deps right
            New-ReleaseFact -Name right -Deps left
        )

        {
            Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'targeted'
                tokens = @('left@patch')
                classifications = @{ left = 'patch'; right = 'patch' }
            }
        } | Should -Throw '*dependency cycle*'
    }
}

Describe 'resolve-plan.ps1 external dependency exposure' {
    It 'blocks a breaking exposed dependency bump declared as a patch' {
        # The run-8 shape for manifests: nothing in the crate's own rustdoc
        # moved, so cargo-semver-checks reports patch, while every consumer that
        # names syn::Error now sees a different type.
        $facts = @(
            New-ReleaseFact -Name macro_impl `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macro_impl')
            classifications = @{ macro_impl = 'patch' }
        }

        $plan.status | Should -Be 'blocked'
        $plan.releases.Count | Should -Be 0
        @($plan.ambiguities | ForEach-Object { $_.kind }) |
            Should -Contain 'externalExposureUnderclassified'
    }

    It 'blocks a nonbreaking classification just as it blocks a patch' {
        $facts = @(
            New-ReleaseFact -Name macro_impl `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macro_impl')
            classifications = @{ macro_impl = 'nonbreaking' }
        }

        $plan.status | Should -Be 'blocked'
        $plan.releases.Count | Should -Be 0
    }

    It 'reports the dependency, both requirements and the derived floor' {
        $facts = @(
            New-ReleaseFact -Name macro_impl `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macro_impl')
            classifications = @{ macro_impl = 'patch' }
        }

        $ambiguity = @(
            $plan.ambiguities |
                Where-Object { $_.kind -eq 'externalExposureUnderclassified' }
        )[0]
        $ambiguity.package | Should -Be 'macro_impl'
        $ambiguity.classified | Should -Be 'patch'
        $ambiguity.derivedFloor | Should -Be 'breaking'
        $ambiguity.dependencies[0].name | Should -Be 'syn'
        $ambiguity.dependencies[0].baselineReq | Should -Be '^2.0.111'
        $ambiguity.dependencies[0].currentReq | Should -Be '^3.0.2'
        $ambiguity.requiredInput | Should -Be 'classifications.macro_impl'
    }

    It 'resolves once the classification meets the derived floor' {
        $facts = @(
            New-ReleaseFact -Name macro_impl -Version '0.2.0' `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macro_impl')
            classifications = @{ macro_impl = 'breaking' }
        }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].folder | Should -Be 'macro_impl'
        $plan.releases[0].to | Should -Be '0.3.0'
    }

    It 'does not break on a private external dependency bump' {
        $facts = @(
            New-ReleaseFact -Name private_user -Version '1.2.3' `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('serde')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('private_user')
            classifications = @{ private_user = 'patch' }
        }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].to | Should -Be '1.2.4'
    }

    It 'does not break on a non-breaking requirement change to an exposed dependency' {
        $facts = @(
            New-ReleaseFact -Name exposer -Version '1.2.3' `
                -ExternalDepChanges @(
                    New-ExternalDepChange -Name syn `
                        -BaselineReq '^2.0.111' -CurrentReq '^2.9.0' -Breaking $false
                ) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('exposer')
            classifications = @{ exposer = 'patch' }
        }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].to | Should -Be '1.2.4'
    }

    It 'does not break a proc macro, whose exposure set is always empty' {
        $facts = @(
            New-ReleaseFact -Name macros -Version '0.4.0' -ProcMacroOnly $true `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @()
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macros')
            classifications = @{ macros = 'patch' }
            macroContracts = @{ macros = New-MacroContract -ReviewedPackages @('macros') }
        }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].to | Should -Be '0.4.1'
    }

    It 'never floors a crate that has never been released' {
        $facts = @(
            New-ReleaseFact -Name newcomer -Version '0.1.0' -EverReleased $false `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('newcomer')
            classifications = @{}
        }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].to | Should -Be '0.1.0'
    }

    It 'never floors on a dropped dependency, which cannot be exposed any more' {
        $facts = @(
            New-ReleaseFact -Name dropper -Version '1.2.3' `
                -ExternalDepChanges @(
                    New-ExternalDepChange -Name anyhow `
                        -BaselineReq '^1.0.100' -CurrentReq $null -Breaking $true
                ) `
                -ExternalExposedDeps @('serde')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('dropper')
            classifications = @{ dropper = 'patch' }
        }

        $plan.status | Should -Be 'resolved'
        $plan.releases[0].to | Should -Be '1.2.4'
    }

    It 'cascades the derived break to workspace dependents that expose it' {
        $facts = @(
            New-ReleaseFact -Name macro_impl -Version '0.2.0' `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
            New-ReleaseFact -Name facade -Version '0.2.0' -Deps macro_impl `
                -ExposedDeps macro_impl -Modified $false
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('macro_impl')
            classifications = @{ macro_impl = 'breaking'; facade = 'patch' }
        }

        $plan.status | Should -Be 'resolved'
        $byPackage = @{}
        foreach ($release in $plan.releases) { $byPackage[$release.folder] = $release }
        $byPackage['macro_impl'].to | Should -Be '0.3.0'
        $byPackage['facade'].to | Should -Be '0.3.0'
    }

    It 'blocks a dependent that carries its own exposed break at a patch floor' {
        $facts = @(
            New-ReleaseFact -Name core -Version '0.2.0'
            New-ReleaseFact -Name dependent -Version '0.2.0' -Deps core `
                -ExposedDeps core -Modified $false `
                -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                -ExternalExposedDeps @('syn')
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'breaking'; dependent = 'patch' }
        }

        $plan.status | Should -Be 'blocked'
        $plan.releases.Count | Should -Be 0
        @($plan.ambiguities | ForEach-Object { $_.kind }) |
            Should -Contain 'externalExposureUnderclassified'
    }

    Context 'selection reason coupling' {
        It 'blocks a declined package whose exposed dependency break is real' {
            $facts = @(
                New-ReleaseFact -Name macro_impl `
                    -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                    -ExternalExposedDeps @('syn')
            )
            $plan = Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'changed'
                tokens = @()
                classifications = @{ macro_impl = 'patch' }
                selectionDecisions = @{
                    macro_impl = New-SelectionDecision `
                        -Decision 'decline' -Reason 'internal-only'
                }
            }

            $plan.status | Should -Be 'blocked'
            $plan.releases.Count | Should -Be 0
            @($plan.ambiguities | ForEach-Object { $_.kind }) |
                Should -Contain 'externalExposureUnderselected'
        }

        It 'blocks an accepted package whose reason is softer than the derived floor' {
            $facts = @(
                New-ReleaseFact -Name macro_impl `
                    -ManifestDependencyScopes @('normal') `
                    -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                    -ExternalExposedDeps @('syn')
            )
            $plan = Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'changed'
                tokens = @('macro_impl')
                classifications = @{ macro_impl = 'breaking' }
                selectionDecisions = @{
                    macro_impl = New-SelectionDecision `
                        -Decision 'accept' -Reason 'runtime-manifest-change'
                }
            }

            $plan.status | Should -Be 'blocked'
            $plan.releases.Count | Should -Be 0
            $ambiguity = @(
                $plan.ambiguities |
                    Where-Object { $_.kind -eq 'externalExposureUnderselected' }
            )[0]
            $ambiguity.reason | Should -Be 'runtime-manifest-change'
            $ambiguity.derivedFloor | Should -Be 'breaking'
            $ambiguity.requiredInput | Should -Be 'selectionDecisions.macro_impl.reason'
        }

        It 'resolves when both the reason and the classification meet the floor' {
            $facts = @(
                New-ReleaseFact -Name macro_impl -Version '0.2.0' `
                    -ExternalDepChanges @(New-ExternalDepChange -Name syn) `
                    -ExternalExposedDeps @('syn')
            )
            $plan = Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'changed'
                tokens = @('macro_impl')
                classifications = @{ macro_impl = 'breaking' }
                selectionDecisions = @{
                    macro_impl = New-SelectionDecision -Decision 'accept' -Reason 'breaking'
                }
            }

            $plan.status | Should -Be 'resolved'
            $plan.releases[0].to | Should -Be '0.3.0'
        }

        It 'leaves an unaffected package free to decline' {
            $facts = @(
                New-ReleaseFact -Name plain `
                    -ExternalDepChanges @(
                        New-ExternalDepChange -Name syn -Breaking $false `
                            -BaselineReq '^2.0.111' -CurrentReq '^2.9.0'
                    ) `
                    -ExternalExposedDeps @('syn')
            )
            $plan = Invoke-ReleasePlan -Facts $facts -Request @{
                mode = 'changed'
                tokens = @()
                classifications = @{ plain = 'patch' }
                selectionDecisions = @{
                    plain = New-SelectionDecision -Decision 'decline' -Reason 'internal-only'
                }
            }

            $plan.status | Should -Be 'resolved'
            $plan.releases.Count | Should -Be 0
        }
    }

    It 'rejects facts that predate the external dependency lane' {
        $fact = New-ReleaseFact -Name package
        $fact.Remove('externalDepChanges')

        {
            Invoke-ReleasePlan -Facts @($fact) -Request @{
                mode = 'targeted'
                tokens = @('package@patch')
                classifications = @{ package = 'patch' }
            }
        } | Should -Throw "*missing 'externalDepChanges'*"
    }
}
