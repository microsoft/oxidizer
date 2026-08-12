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
            # Pins the manifest against the literal asserted in
            # PureFunctions.Tests.ps1. Deleting `bytesbuf::*` here is precisely
            # how the fail-open would be reintroduced.
            $allowed = @((Get-LivePackage -Folder 'bytesbuf_io').AllowedExternalTypes)
            $allowed | Should -Not -BeNullOrEmpty -Because 'absent metadata would mask the real assertion behind the fail-closed branch'

            $roots = @($allowed | ForEach-Object { ($_ -split '::', 2)[0] })
            $roots | Should -Contain 'bytesbuf'
        }

        It 'still matches every shared allowlist literal entry' {
            # The unit tests assert exposure of `ohno` and `futures_core` too,
            # using the same literal. Checking only the bytesbuf root would
            # leave those two asserted against a copy nothing pins, so the unit
            # tests could keep passing on entries the manifest had dropped.
            # Compare the whole set, which is what makes the "copied verbatim"
            # comment in _common a fact rather than an intention.
            $allowed = @((Get-LivePackage -Folder 'bytesbuf_io').AllowedExternalTypes)

            # Order is irrelevant -- this pins the contents of the allowlist,
            # not how the manifest happens to sort them.
            ($allowed | Sort-Object) -join '|' |
                Should -Be ((Get-BytesBufIoAllowlist | Sort-Object) -join '|') `
                -Because 'crates/bytesbuf_io/Cargo.toml and Get-BytesBufIoAllowlist must not drift apart'
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

Describe 'Re-exported type edges in the live workspace' {
    # cargo-check-external-types attributes a re-exported type to its DEFINING
    # crate, so a crate can allowlist a workspace crate it does not directly
    # depend on. These edges exist today and were invisible to a direct-edge
    # scan. Pinning them against real manifests, because the whole failure mode
    # is a mismatch between what manifests say and what the planner assumed.

    BeforeAll {
        # Every (crate, allowlisted workspace crate) pair where the crate does
        # not depend on that workspace crate directly. Self-references are
        # excluded: a crate cannot cascade from itself.
        $script:IndirectPairs = @()
        $byName = @{}
        foreach ($p in $script:LiveBaseline) { $byName[$p.Name.Replace('-', '_')] = $p }

        foreach ($pkg in $script:LiveBaseline) {
            if ($null -eq $pkg.AllowedExternalTypes) { continue }
            $self = $pkg.Name.Replace('-', '_')
            $roots = @($pkg.AllowedExternalTypes |
                    Where-Object { $_ -is [string] -and -not [string]::IsNullOrWhiteSpace($_) } |
                    ForEach-Object { ($_ -split '::', 2)[0] } | Sort-Object -Unique)
            foreach ($root in $roots) {
                if (-not $byName.ContainsKey($root)) { continue }
                if ($root -eq $self) { continue }
                if ($pkg.Deps -contains $root) { continue }
                $script:IndirectPairs += [pscustomobject]@{ Dependent = $pkg; TargetName = $byName[$root].Name }
            }
        }
    }

    It 'still contains at least one indirect allowlist edge to pin' {
        # If this ever fails the workspace changed shape; the tests below would
        # then be silently vacuous, so fail loudly instead.
        @($script:IndirectPairs).Count | Should -BeGreaterThan 0
    }

    It 'treats every indirect allowlist edge as exposure of the defining crate' {
        foreach ($pair in $script:IndirectPairs) {
            Test-PackageAllowlistNamesTarget -Dependent $pair.Dependent -TargetPackageName $pair.TargetName |
                Should -BeTrue -Because "$($pair.Dependent.Folder) allowlists $($pair.TargetName) without depending on it directly"
        }
    }

    It 'selects those crates as dependents of the defining crate' {
        # The direct-edge scan could not: none of these appear in the target's
        # direct dependent list at all.
        foreach ($pair in $script:IndirectPairs) {
            $target = $script:LiveBaseline | Where-Object { $_.Name -eq $pair.TargetName }
            $resolvedStub = [ordered]@{}
            foreach ($p in $script:LiveBaseline) { $resolvedStub[$p.Folder] = $true }

            $selected = @(Get-PublishedDependentsExposingTarget -TargetPackage $target `
                    -WorkspaceBaseline $script:LiveBaseline -Resolved $resolvedStub)

            @($selected | Where-Object { $_.Folder -eq $pair.Dependent.Folder }).Count |
                Should -Be 1 -Because "$($pair.Dependent.Folder) reaches $($pair.TargetName) and names its types"
        }
    }

    It 'cascades a breaking bump of each defining crate into those dependents' {
        $targets = @($script:IndirectPairs | ForEach-Object { $_.TargetName } | Sort-Object -Unique)
        foreach ($targetName in $targets) {
            $resolved = Resolve-ReleaseSet `
                -ParsedTokens (Parse-ReleaseTokens -Tokens @("$targetName@breaking")) `
                -WorkspaceBaseline $script:LiveBaseline `
                -GetRequiredChangeType $script:NoSelfChangeClassifier
            $byFolder = @{}
            foreach ($entry in $resolved) { $byFolder[$entry.Folder] = $entry }

            $expected = @($script:IndirectPairs | Where-Object { $_.TargetName -eq $targetName })
            foreach ($pair in $expected) {
                $folder = $pair.Dependent.Folder
                $byFolder.ContainsKey($folder) | Should -BeTrue -Because "$folder depends on $targetName transitively"
                $byFolder[$folder].EffectiveChangeType |
                    Should -Be 'breaking' -Because "$folder names $targetName's types in its public API"
            }
        }
    }
}
