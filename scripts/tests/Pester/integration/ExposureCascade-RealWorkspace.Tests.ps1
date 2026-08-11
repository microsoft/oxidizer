# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# End-to-end regression coverage for the exposed-dependency cascade, run
# against the LIVE workspace rather than synthetic package records.
#
# Every other test of this logic builds its own [pscustomobject] baseline, so
# the planner is only ever proven correct about graphs the test itself
# invented. That leaves the real failure mode unpinned: bytesbuf_io exposes
# bytesbuf types across its public API, so a breaking bytesbuf release must
# force a breaking bytesbuf_io release. Before this cascade existed,
# bytesbuf_io took a mechanical patch floor and shipped a silent break.
#
# These tests read the actual manifests. That coupling is deliberate: if the
# bytesbuf/bytesbuf_io relationship changes, or bytesbuf_io's allowlist is
# edited, this file must fail and be consciously updated. A snapshot fixture
# would re-create exactly the staleness problem it is meant to catch.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')

    $script:RepoRoot = Get-OxiRepoRoot
    # cargo metadata over the whole workspace is slow; resolve it once.
    $script:LiveBaseline = @(Get-WorkspacePackages -repoRoot $script:RepoRoot)

    function Get-LivePackage {
        param([string]$Folder)
        return $script:LiveBaseline | Where-Object { $_.Folder -eq $Folder }
    }

    # Keeps the planner off cargo-semver-checks: every crate reports 'none', so
    # the only thing that can raise a version is the cascade under test.
    $script:NoSelfChangeClassifier = {
        param([string]$Folder, [string]$CargoName)
        return 'none'
    }
}

Describe 'Exposed-dependency cascade over the live workspace' {
    Context 'the bytesbuf -> bytesbuf_io topology' {
        It 'still has both crates present and published' {
            foreach ($folder in @('bytesbuf', 'bytesbuf_io')) {
                $pkg = Get-LivePackage -Folder $folder
                $pkg | Should -Not -BeNullOrEmpty -Because "crates/$folder underpins this regression test"
                $pkg.Published | Should -BeTrue -Because "an unpublished $folder would drop out of the cascade entirely"
            }
        }

        It 'still records bytesbuf as a non-dev dependency of bytesbuf_io' {
            # The edge itself. Without it the cascade never even considers the
            # pair, and every assertion below would pass vacuously.
            (Get-LivePackage -Folder 'bytesbuf_io').Deps | Should -Contain 'bytesbuf'
        }

        It 'still declares a bytesbuf-rooted entry in the real bytesbuf_io allowlist' {
            # Pins the manifest against the literal allowlist asserted in
            # PureFunctions.Tests.ps1. Deleting `bytesbuf::*` here is precisely
            # how the fail-open would be reintroduced.
            $allowed = @((Get-LivePackage -Folder 'bytesbuf_io').AllowedExternalTypes)
            $allowed | Should -Not -BeNullOrEmpty -Because 'absent metadata would mask the real assertion behind the fail-closed branch'

            $roots = @($allowed | ForEach-Object { ($_ -split '::', 2)[0] })
            $roots | Should -Contain 'bytesbuf'
        }

        It 'reports bytesbuf_io as exposing bytesbuf using the real package records' {
            Test-PackageExposesTarget `
                -Dependent (Get-LivePackage -Folder 'bytesbuf_io') `
                -TargetPackageName 'bytesbuf' | Should -BeTrue
        }
    }

    Context 'planning a breaking bytesbuf release' {
        BeforeAll {
            $parsed = Parse-ReleaseTokens -Tokens @('bytesbuf@breaking')
            $resolved = Resolve-ReleaseSet `
                -ParsedTokens $parsed `
                -WorkspaceBaseline $script:LiveBaseline `
                -GetRequiredChangeType $script:NoSelfChangeClassifier

            $script:ByFolder = @{}
            foreach ($entry in $resolved) { $script:ByFolder[$entry.Folder] = $entry }
        }

        It 'pulls bytesbuf_io into the release set' {
            $script:ByFolder.ContainsKey('bytesbuf_io') | Should -BeTrue
        }

        It 'raises bytesbuf_io to breaking rather than leaving it on the patch floor' {
            # THE regression. A 'patch' here is the original bug: a compatible
            # release that silently breaks every bytesbuf_io consumer.
            $script:ByFolder['bytesbuf_io'].EffectiveChangeType | Should -Be 'breaking'
        }

        It 'writes a version for bytesbuf_io that is an incompatible transition' {
            $entry = $script:ByFolder['bytesbuf_io']
            # Asserted via the change-type calculation rather than a hardcoded
            # version so routine bytesbuf_io releases do not break this test.
            $planned = Get-ChangeTypeFromVersions `
                -oldVersion $entry.CurrentVersion `
                -newVersion $entry.EffectiveTargetVersion
            Test-IsBreakingChange -oldVersion $entry.CurrentVersion -ChangeType $planned |
                Should -BeTrue -Because "$($entry.CurrentVersion) -> $($entry.EffectiveTargetVersion) must be incompatible"
        }

        It 'attributes the bytesbuf_io bump to bytesbuf' {
            $reason = @($script:ByFolder['bytesbuf_io'].CascadeReasons | Where-Object { $_.Target -eq 'bytesbuf' })
            $reason.Count       | Should -Be 1 -Because 'one reason per edge, however many fixpoint passes run'
            $reason[0].Breaking | Should -BeTrue
        }
    }

    Context 'planning a compatible bytesbuf release' {
        It 'does not raise bytesbuf_io to breaking when bytesbuf stays compatible' {
            # The negative control: exposure alone must not force a break. If
            # this fails, the cascade is bumping on the edge rather than on the
            # incompatibility, and every release becomes a major.
            $parsed = Parse-ReleaseTokens -Tokens @('bytesbuf@patch')
            $resolved = Resolve-ReleaseSet `
                -ParsedTokens $parsed `
                -WorkspaceBaseline $script:LiveBaseline `
                -GetRequiredChangeType $script:NoSelfChangeClassifier

            $entry = $resolved | Where-Object { $_.Folder -eq 'bytesbuf_io' }
            $entry.EffectiveChangeType | Should -Not -Be 'breaking'
        }
    }
}
