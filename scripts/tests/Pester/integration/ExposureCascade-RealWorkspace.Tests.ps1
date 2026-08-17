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
            # This test exercises cascade mechanics with macro contracts held
            # constant, so every compile-fixture obligation the facts report is
            # discharged with an unchanged outcome. Mirroring the two sides is
            # what "held constant" means mechanically; a real review measures
            # them instead.
            $compileEvidence = @(
                foreach ($obligation in @($fact.macroCompileFixtureChanges)) {
                    $result = if ($obligation.expectedResult -eq 'fail') {
                        'fail'
                    } else {
                        'pass'
                    }
                    $exitCode = if ($result -eq 'fail') { 101 } else { 0 }
                    @{
                        ownerPackage = $obligation.ownerPackage
                        path = $obligation.path
                        baseline = @{
                            result = $result
                            revision = $obligation.baselineRev
                            exitCode = $exitCode
                        }
                        current = @{
                            result = $result
                            revision = 'HEAD'
                            exitCode = $exitCode
                        }
                    }
                }
            )
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
                compileEvidence = $compileEvidence
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

    Context 'external dependency exposure' {
        It 'reports syn exposure for the macro implementation crates that name its types' {
            foreach ($folder in @(
                    'thread_aware_macros_impl',
                    'data_privacy_macros_impl',
                    'fundle_macros_impl'
                )) {
                @($script:FactsByFolder[$folder].externalExposedDeps) |
                    Should -Contain 'syn' -Because "$folder allowlists syn:: entries"
            }
        }

        It 'reports no syn exposure for crates that only use it privately' {
            foreach ($folder in @('templated_uri_macros_impl', 'routerama_build')) {
                @($script:FactsByFolder[$folder].externalExposedDeps) |
                    Should -Not -Contain 'syn' -Because "$folder allowlists no syn:: entry"
            }
        }

        It 'never reports external exposure for a proc-macro-only crate' {
            foreach ($fact in $script:Facts.packages) {
                if (-not [bool]$fact.procMacroOnly) { continue }
                @($fact.externalExposedDeps).Count |
                    Should -Be 0 -Because "$($fact.folder) exports behaviour, not foreign types"
            }
        }

        It 'emits both lane properties for every workspace package' {
            foreach ($fact in $script:Facts.packages) {
                $fact.PSObject.Properties['externalDepChanges'] |
                    Should -Not -BeNullOrEmpty
                $fact.PSObject.Properties['externalExposedDeps'] |
                    Should -Not -BeNullOrEmpty
            }
        }

        It 'reports only external crates, never workspace members' {
            $members = @($script:Facts.packages | ForEach-Object { $_.name.Replace('-', '_') })
            foreach ($fact in $script:Facts.packages) {
                foreach ($change in @($fact.externalDepChanges)) {
                    $members | Should -Not -Contain $change.name
                }
            }
        }

        It 'orders every package lane deterministically' {
            foreach ($fact in $script:Facts.packages) {
                $names = @($fact.externalDepChanges | ForEach-Object { $_.name })
                $sorted = [string[]]@($names)
                [Array]::Sort($sorted, [StringComparer]::Ordinal)
                $names | Should -Be $sorted
            }
        }

        It 'blocks a patch plan for any crate whose exposed dependency line moved' {
            $floored = @(
                foreach ($fact in $script:Facts.packages) {
                    if (-not [bool]$fact.published) { continue }
                    $exposed = @($fact.externalExposedDeps)
                    $breaking = @(
                        @($fact.externalDepChanges) |
                            Where-Object { [bool]$_.breaking -and $exposed -contains $_.name }
                    )
                    if ($breaking.Count -gt 0) { $fact }
                }
            )
            if ($floored.Count -eq 0) {
                Set-ItResult -Skipped -Because 'no exposed external break is pending in this tree'
                return
            }

            # Invoke-LivePlan classifies every published package as patch, which
            # is exactly the judgement the derived floor has to refuse.
            $plan = Invoke-LivePlan -Token $floored[0].folder
            $plan.status | Should -Be 'blocked'
            @($plan.releases).Count | Should -Be 0
            @($plan.ambiguities | ForEach-Object { $_.kind }) |
                Should -Contain 'externalExposureUnderclassified'
        }
    }
}
