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
            workspaceModified = $WorkspaceModified
        }
    }

    function New-MacroContract {
        param(
            [ValidateSet('compatible', 'nonbreaking', 'breaking')]
            [string]$Verdict = 'compatible',
            [string[]]$ReviewedPackages = @('macros'),
            [string[]]$Evidence = @('Reviewed macro exports, compile fixtures, and generated API.')
        )

        return @{
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
    }

    function Invoke-ReleasePlan {
        param(
            [Parameter(Mandatory = $true)][object[]]$Facts,
            [Parameter(Mandatory = $true)][hashtable]$Request,
            [int]$SchemaVersion = 4
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

    function New-SelectionDecision {
        param(
            [ValidateSet('accept', 'decline')]
            [string]$Decision = 'accept',
            [string]$Reason = 'behavior-fix'
        )

        return @{
            decision = $Decision
            reason = $Reason
            evidence = @('Reviewed the package diff from its release baseline.')
        }
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
                New-ReleaseFact -Name declined
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
