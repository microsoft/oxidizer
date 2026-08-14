# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')

    $script:RepoRoot = Get-OxiRepoRoot
    $script:FactsScript = Join-Path (
        $script:RepoRoot
    ) '.github\skills\release-packages\scripts\release-facts.ps1'
    $script:Resolver = Join-Path (
        $script:RepoRoot
    ) '.github\skills\release-packages\scripts\resolve-plan.ps1'

    $script:Facts = & $script:FactsScript -RepoRoot $script:RepoRoot |
        ConvertFrom-Json
    $script:FactsByFolder = @{}
    $script:FactsByName = @{}
    foreach ($fact in $script:Facts.packages) {
        $script:FactsByFolder[$fact.folder] = $fact
        $script:FactsByName[$fact.name.Replace('-', '_')] = $fact
    }

    function Invoke-LivePlan {
        param([Parameter(Mandatory = $true)][string]$Token)

        $caseDir = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $caseDir | Out-Null
        $factsPath = Join-Path $caseDir 'facts.json'
        $requestPath = Join-Path $caseDir 'request.json'
        $factsForPlan = $script:Facts |
            ConvertTo-Json -Depth 8 |
            ConvertFrom-Json
        foreach ($fact in $factsForPlan.packages) {
            if ([bool]$fact.published) {
                # CI checks out pull-request merge refs without tags. This live
                # topology test exercises cascade mechanics, not tag discovery.
                $fact.everReleased = $true
            }
        }
        $factsForPlan |
            ConvertTo-Json -Depth 8 |
            Set-Content -LiteralPath $factsPath -Encoding utf8
        $classifications = @{}
        foreach ($fact in $factsForPlan.packages) {
            if ([bool]$fact.published) {
                $classifications[$fact.folder] = 'patch'
            }
        }
        $allFolders = @($factsForPlan.packages.folder)
        $macroContracts = @{}
        foreach ($fact in $factsForPlan.packages) {
            if (-not [bool]$fact.procMacroOnly) { continue }
            $macroContracts[$fact.folder] = @{
                verdict = 'compatible'
                reviewedPackages = $allFolders
                channels = @{
                    exportedMacros = 'unchanged'
                    acceptedSyntax = 'unchanged'
                    compileBehavior = 'unchanged'
                    generatedApi = 'unchanged'
                    generatedRuntimePaths = 'unchanged'
                    hygiene = 'unchanged'
                }
                evidence = @('Live topology tests hold macro contracts constant.')
            }
        }
        @{
            mode = 'targeted'
            tokens = @($Token)
            classifications = $classifications
            macroContracts = $macroContracts
        } |
            ConvertTo-Json -Depth 8 |
            Set-Content -LiteralPath $requestPath -Encoding utf8

        & $script:Resolver `
            -FactsPath $factsPath `
            -RequestPath $requestPath |
            ConvertFrom-Json
    }
}

Describe 'Exposure cascades over the live workspace' {
    It 'identifies bytesbuf exposure in bytesbuf_io facts' {
        $bytesbufIo = $script:FactsByFolder['bytesbuf_io']

        @($bytesbufIo.deps) | Should -Contain 'bytesbuf'
        @($bytesbufIo.exposedDeps) | Should -Contain 'bytesbuf'
    }

    It 'raises bytesbuf_io to breaking for a breaking bytesbuf release' {
        $plan = Invoke-LivePlan -Token 'bytesbuf@breaking'
        $bytesbufIo = $plan.releases |
            Where-Object folder -eq 'bytesbuf_io'

        $bytesbufIo.changeType | Should -Be 'breaking'
        @($bytesbufIo.cascadeReasons |
                Where-Object { $_.target -eq 'bytesbuf' -and $_.breaking }).Count |
            Should -Be 1
    }

    It 'finds at least one indirect public exposure edge' {
        $indirect = @(
            foreach ($fact in $script:Facts.packages) {
                foreach ($target in @($fact.exposedDeps)) {
                    if (@($fact.deps) -notcontains $target) {
                        [pscustomobject]@{ Dependent = $fact; Target = $target }
                    }
                }
            }
        )

        $indirect.Count | Should -BeGreaterThan 0
    }

    It 'raises an indirect exposing package when the defining crate breaks' {
        $pair = @(
            foreach ($fact in $script:Facts.packages) {
                if (-not [bool]$fact.published) { continue }
                foreach ($target in @($fact.exposedDeps)) {
                    if (
                        @($fact.deps) -notcontains $target -and
                        $script:FactsByName.ContainsKey($target)
                    ) {
                        [pscustomobject]@{
                            Dependent = $fact
                            Target = $script:FactsByName[$target]
                        }
                    }
                }
            }
        ) | Select-Object -First 1

        $pair | Should -Not -BeNullOrEmpty
        $plan = Invoke-LivePlan -Token "$($pair.Target.folder)@breaking"
        $dependent = $plan.releases |
            Where-Object folder -eq $pair.Dependent.folder

        $dependent.changeType | Should -Be 'breaking'
        @($dependent.cascadeReasons |
                Where-Object { $_.target -eq $pair.Target.name -and $_.breaking }).Count |
            Should -Be 1
    }

    It 'separates the templated-uri macro contract from Rust type exposure' {
        $impl = $script:FactsByFolder['templated_uri_macros_impl']
        $macros = $script:FactsByFolder['templated_uri_macros']
        $runtime = $script:FactsByFolder['templated_uri']

        @($impl.exposedDeps) | Should -Not -Contain 'ohno'
        $macros.exposureUnknown | Should -BeFalse
        @($macros.exposedDeps).Count | Should -Be 0
        @($runtime.macroPublicDeps) | Should -Contain 'templated_uri_macros'
        @($runtime.exposedDeps) | Should -Not -Contain 'templated_uri_macros'
        @($macros.macroRuntimePartners) | Should -Contain 'templated_uri'
    }

    It 'does not turn wildcard or unpublished macro consumers into runtime partners' {
        $ohnoMacros = $script:FactsByFolder['ohno_macros']
        $routeramaMacros = $script:FactsByFolder['routerama_macros']

        @($ohnoMacros.macroRuntimePartners) | Should -Not -Contain 'automation'
        @($routeramaMacros.macroRuntimePartners) |
            Should -Not -Contain 'rest_over_grpc_examples'
        @($routeramaMacros.macroRuntimePartners) |
            Should -Not -Contain 'rest_over_grpc_tests'
    }

    It 'keeps compatible templated-uri macro packages at patch while preserving independent type breaks' {
        $plan = Invoke-LivePlan -Token 'ohno@breaking'
        $impl = $plan.releases |
            Where-Object folder -eq 'templated_uri_macros_impl'
        $macros = $plan.releases |
            Where-Object folder -eq 'templated_uri_macros'
        $runtime = $plan.releases |
            Where-Object folder -eq 'templated_uri'

        $impl.changeType | Should -Be 'patch'
        $macros.changeType | Should -Be 'patch'
        $runtime.changeType | Should -Be 'breaking'
    }
}
