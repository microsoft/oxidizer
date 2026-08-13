# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')

    $script:Resolver = Join-Path (
        Get-OxiRepoRoot
    ) '.github\skills\release-packages\scripts\resolve-plan.ps1'

    function New-ReleaseFact {
        param(
            [Parameter(Mandatory = $true)][string]$Name,
            [string]$Version = '1.0.0',
            [string[]]$Deps = @(),
            [string[]]$ExposedDeps = @(),
            [bool]$ExposureUnknown = $false,
            [bool]$Published = $true,
            [bool]$EverReleased = $true,
            [bool]$ProcMacroOnly = $false
        )

        return [ordered]@{
            folder           = $Name
            name             = $Name
            version          = $Version
            published        = $Published
            procMacroOnly    = $ProcMacroOnly
            hasLibraryTarget = -not $ProcMacroOnly
            deps             = @($Deps)
            exposedDeps      = @($ExposedDeps)
            exposureUnknown  = $ExposureUnknown
            baselineSha      = if ($EverReleased) { '0123456789012345678901234567890123456789' } else { $null }
            hasBaseline      = $EverReleased
            everReleased     = $EverReleased
            modified         = $true
            modifiedFileCount = 1
        }
    }

    function Invoke-ReleasePlan {
        param(
            [Parameter(Mandatory = $true)][object[]]$Facts,
            [Parameter(Mandatory = $true)][hashtable]$Request
        )

        $caseDir = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $caseDir | Out-Null
        $factsPath = Join-Path $caseDir 'facts.json'
        $requestPath = Join-Path $caseDir 'request.json'
        [ordered]@{ packages = @($Facts) } |
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

    It 'keeps proc-macro manual review visible when a cascade becomes breaking' {
        $facts = @(
            New-ReleaseFact -Name core
            New-ReleaseFact -Name macros -Version '0.4.0' -Deps core -ExposedDeps core -ProcMacroOnly $true
        )
        $plan = Invoke-ReleasePlan -Facts $facts -Request @{
            mode = 'targeted'
            tokens = @('core@breaking')
            classifications = @{ core = 'patch' }
        }

        $macros = $plan.releases | Where-Object folder -eq macros
        $macros.to | Should -Be '0.5.0'
        $macros.changeType | Should -Be 'breaking'
        $macros.manualReview | Should -BeTrue
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

    It 'preserves changed and all mode labels after package selection' {
        foreach ($mode in @('changed', 'all')) {
            $plan = Invoke-ReleasePlan `
                -Facts @(New-ReleaseFact -Name package) `
                -Request @{
                    mode = $mode
                    tokens = @('package')
                    classifications = @{ package = 'patch' }
                }
            $plan.mode | Should -Be $mode
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
