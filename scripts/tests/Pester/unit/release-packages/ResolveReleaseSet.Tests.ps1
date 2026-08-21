# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')

    # Helper that builds a baseline package record. Underscore-only cargo
    # names by default so the test stays focused on the cascade/resolve logic
    # rather than name normalization.
    #
    # AllowedExternalTypes defaults to @() -- "this crate's public API names
    # nothing foreign" -- and NOT to $null. $null is the fail-closed branch
    # (absent metadata => assume exposure), so defaulting to it would make
    # every package in every baseline expose every dependency for free. That
    # masks the signal under test: a cascade assertion would pass on the
    # fallback even when the behaviour it is meant to pin is broken. @() is
    # inert, so a test that needs exposure has to ask for it -- either with a
    # real allowlist entry or by passing $null explicitly.
    function New-BaselinePackage {
        param(
            [string]   $Folder,
            [string]   $Name = $null,
            [string]   $Version = '0.1.0',
            [string[]] $Deps = @(),
            [bool]     $Published = $true,
            [bool]     $IsProcMacroOnly = $false,
            [hashtable] $DepAliases = @{},
            # The crate's rustdoc name -- its [lib] name, which defaults to the
            # normalized package name. Allowlist entries earned on an INDIRECT
            # path are rooted at this, never at a rename alias: a rename exists
            # only on an edge the renaming crate declares, and an indirect path
            # crosses no such edge to the target.
            [string]   $CrateRoot = $null,
            [AllowNull()][string[]] $AllowedExternalTypes = @()
        )
        if ([string]::IsNullOrEmpty($Name)) { $Name = $Folder }
        if ([string]::IsNullOrEmpty($CrateRoot)) { $CrateRoot = $Name.Replace('-', '_') }
        return [pscustomobject]@{
            Folder    = $Folder
            Name      = $Name
            Version   = $Version
            Published = $Published
            Deps      = $Deps
            DepAliases = $DepAliases
            CrateRoot = $CrateRoot
            IsProcMacroOnly = $IsProcMacroOnly
            AllowedExternalTypes = $AllowedExternalTypes
        }
    }

    # Builds a stub cargo-semver-checks classifier from a folder -> change-type
    # map. Unmapped folders return 'none' (no constraint). Lets the cascade /
    # self-floor logic be tested deterministically without invoking the real
    # tool. In production the classifier is $script:DefaultSemverClassifier, which
    # calls Get-CrateRequiredChangeType (a cached cargo-semver-checks wrapper).
    function New-StubClassifier {
        param([hashtable]$Map = @{})
        return {
            param([string]$Folder, [string]$CargoName)
            $t = $Map[$Folder]
            if ($t) { return $t }
            return 'none'
        }.GetNewClosure()
    }

    # Linear baseline: a → b → c → d (each depends on the previous).
    function New-LinearBaseline {
        return @(
            (New-BaselinePackage -Folder 'a' -Version '0.1.0' -Deps @())
            (New-BaselinePackage -Folder 'b' -Version '0.1.0' -Deps @('a'))
            (New-BaselinePackage -Folder 'c' -Version '0.1.0' -Deps @('b'))
            (New-BaselinePackage -Folder 'd' -Version '0.1.0' -Deps @('c'))
        )
    }
}

Describe 'Get-TransitivePublishedDependentsFromBaseline' {
    It 'returns all transitive published dependents in a linear chain' {
        $baseline = New-LinearBaseline
        $result = Get-TransitivePublishedDependentsFromBaseline -Baseline $baseline -TargetCargoName 'a'
        $result | Should -Be @('b', 'c', 'd')
    }

    It 'excludes the target itself' {
        $baseline = New-LinearBaseline
        $result = Get-TransitivePublishedDependentsFromBaseline -Baseline $baseline -TargetCargoName 'b'
        $result | Should -Not -Contain 'b'
        $result | Should -Be @('c', 'd')
    }

    It 'traverses through unpublished packages but does not include them in the result' {
        # a -> b(unpublished) -> c
        $baseline = @(
            (New-BaselinePackage -Folder 'a' -Deps @())
            (New-BaselinePackage -Folder 'b' -Deps @('a') -Published $false)
            (New-BaselinePackage -Folder 'c' -Deps @('b'))
        )
        $result = Get-TransitivePublishedDependentsFromBaseline -Baseline $baseline -TargetCargoName 'a'
        $result | Should -Not -Contain 'b'
        $result | Should -Contain 'c'
    }

    It 'returns an empty result when no package depends on the target' {
        $baseline = @(
            (New-BaselinePackage -Folder 'a' -Deps @())
            (New-BaselinePackage -Folder 'b' -Deps @())
        )
        $result = @(Get-TransitivePublishedDependentsFromBaseline -Baseline $baseline -TargetCargoName 'a')
        $result.Count | Should -Be 0
    }

    It 'returns an empty result for an empty baseline' {
        $result = @(Get-TransitivePublishedDependentsFromBaseline -Baseline @() -TargetCargoName 'a')
        $result.Count | Should -Be 0
    }
}

Describe 'Get-DirectPublishedDependentsFromBaseline' {
    It 'returns only immediate published consumers' {
        # a -> b -> c; private also directly consumes a but is not published.
        $baseline = @(
            (New-BaselinePackage -Folder 'a')
            (New-BaselinePackage -Folder 'b' -Deps @('a'))
            (New-BaselinePackage -Folder 'c' -Deps @('b'))
            (New-BaselinePackage -Folder 'private' -Deps @('a') -Published $false)
        )

        $result = Get-DirectPublishedDependentsFromBaseline -Baseline $baseline -TargetCargoName 'a'

        $result | Should -Be @('b')
    }
}

Describe 'Resolve-ReleaseSet' {
    Context 'single user-source entry without dependents' {
        It 'returns a single user-source entry with the right effective state (0.x non-breaking -> 0.y.(z+1))' {
            # 0.x.y SemVer: non-breaking is numerically the same as patch
            # (0.y.(z+1)). Get-NextVersion handles this; we just assert the
            # surfaced semantics here.
            $baseline = @((New-BaselinePackage -Folder 'standalone' -Version '0.4.1'))
            $parsed = Parse-ReleaseTokens -Tokens @('standalone@nonbreaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline

            $resolved.Count                         | Should -Be 1
            $resolved[0].Folder                     | Should -Be 'standalone'
            $resolved[0].Source                     | Should -Be 'user'
            $resolved[0].EffectiveChangeType        | Should -Be 'non-breaking'
            $resolved[0].EffectiveTargetVersion     | Should -Be '0.4.2'
            $resolved[0].AutoUpgraded               | Should -BeFalse
            $resolved[0].CascadeReasons.Count       | Should -Be 0
            $resolved[0].RawToken                   | Should -Be 'standalone@nonbreaking'
        }

        It 'computes EffectiveTargetVersion for a 0.x breaking change as 0.(y+1).0' {
            $baseline = @((New-BaselinePackage -Folder 'standalone' -Version '0.4.1'))
            $parsed = Parse-ReleaseTokens -Tokens @('standalone@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $resolved[0].EffectiveTargetVersion | Should -Be '0.5.0'
        }

        It 'computes EffectiveTargetVersion using Get-NextVersion on a 1.x package' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.4.2'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $resolved[0].EffectiveTargetVersion | Should -Be '2.0.0'
        }

        It 'marks a user-selected proc-macro-only package for manual review without invoking the classifier' {
            $baseline = @(
                (New-BaselinePackage -Folder 'macros' -Version '1.0.0' -IsProcMacroOnly $true)
            )
            $classifier = {
                throw 'The automated classifier must not run for proc-macro-only packages.'
            }
            $parsed = Parse-ReleaseTokens -Tokens @('macros@patch')

            $resolved = Resolve-ReleaseSet `
                -ParsedTokens $parsed `
                -WorkspaceBaseline $baseline `
                -GetRequiredChangeType $classifier

            $resolved[0].EffectiveChangeType | Should -Be 'patch'
            $resolved[0].RequiresManualSemverReview | Should -BeTrue
            $resolved[0].IsProcMacroOnly | Should -BeTrue
        }
    }

    Describe 'Get-ManualSemverReviewFindings: breaking review propagation' {
        BeforeAll {
            function script:New-ProcMacroReviewBaseline {
                return @(
                    (New-BaselinePackage -Folder 'macros' -Name 'macros' -Version '1.0.0' -IsProcMacroOnly $true)
                    (New-BaselinePackage -Folder 'facade' -Name 'facade' -Version '1.0.0' -Deps @('macros'))
                    (New-BaselinePackage -Folder 'app' -Name 'app' -Version '1.0.0' -Deps @('facade'))
                )
            }

            function script:Resolve-ToHash {
                param(
                    [object[]]$Baseline,
                    [string[]]$Tokens
                )
                $parsed = Parse-ReleaseTokens -Tokens $Tokens
                $entries = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $Baseline
                $result = @{}
                foreach ($entry in $entries) { $result[$entry.Folder] = $entry }
                return $result
            }
        }

        It 'does not advance beyond an unreviewed proc macro' {
            $baseline = New-ProcMacroReviewBaseline
            $resolved = Resolve-ToHash -Baseline $baseline -Tokens @('macros@breaking')
            $reviewed = [System.Collections.Generic.HashSet[string]]::new()

            $findings = @(Get-ManualSemverReviewFindings `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $baseline `
                -ReviewedManualSemver $reviewed)

            $findings.Folder | Should -Be @('macros')
        }

        It 'surfaces only the direct consumer after the proc macro is reviewed as breaking' {
            $baseline = New-ProcMacroReviewBaseline
            $resolved = Resolve-ToHash -Baseline $baseline -Tokens @('macros@breaking')
            $reviewed = [System.Collections.Generic.HashSet[string]]::new()
            [void]$reviewed.Add('macros')

            $findings = @(Get-ManualSemverReviewFindings `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $baseline `
                -ReviewedManualSemver $reviewed)
            $facade = $findings | Where-Object { $_.Folder -eq 'facade' }

            $facade | Should -Not -BeNullOrEmpty
            $facade.ManualSemverReviewKind | Should -Be 'proc-macro-dependent'
            $facade.ManualSemverReviewSources | Should -Be @('macros')
            $findings.Folder | Should -Not -Contain 'app'
        }

        It 'stops when the reviewed proc macro is non-breaking' {
            $baseline = New-ProcMacroReviewBaseline
            $resolved = Resolve-ToHash -Baseline $baseline -Tokens @('macros@patch')
            $reviewed = [System.Collections.Generic.HashSet[string]]::new()
            [void]$reviewed.Add('macros')

            $findings = @(Get-ManualSemverReviewFindings `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $baseline `
                -ReviewedManualSemver $reviewed)

            $findings.Folder | Should -Be @('macros')
        }

        It 'advances to the next hop only after the direct consumer is reviewed as breaking' {
            $baseline = New-ProcMacroReviewBaseline
            $resolved = Resolve-ToHash -Baseline $baseline -Tokens @('macros@breaking', 'facade@breaking')
            $reviewed = [System.Collections.Generic.HashSet[string]]::new()
            [void]$reviewed.Add('macros')
            [void]$reviewed.Add('facade')

            $findings = @(Get-ManualSemverReviewFindings `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $baseline `
                -ReviewedManualSemver $reviewed)
            $app = $findings | Where-Object { $_.Folder -eq 'app' }

            $app | Should -Not -BeNullOrEmpty
            $app.ManualSemverReviewSources | Should -Be @('facade')
        }

        It 'does not advance when the direct consumer is reviewed below breaking' {
            $baseline = New-ProcMacroReviewBaseline
            $resolved = Resolve-ToHash -Baseline $baseline -Tokens @('macros@breaking', 'facade@patch')
            $reviewed = [System.Collections.Generic.HashSet[string]]::new()
            [void]$reviewed.Add('macros')
            [void]$reviewed.Add('facade')

            $findings = @(Get-ManualSemverReviewFindings `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $baseline `
                -ReviewedManualSemver $reviewed)

            $findings.Folder | Should -Not -Contain 'app'
        }

        It 'follows the actual forced pin instead of a stronger internal severity tag' {
            $baseline = New-ProcMacroReviewBaseline
            $parsed = Parse-ReleaseTokens -Tokens @('macros@breaking', 'facade@1.1.0')
            $classifier = New-StubClassifier @{ facade = 'breaking' }
            $entries = Resolve-ReleaseSet `
                -ParsedTokens $parsed `
                -WorkspaceBaseline $baseline `
                -GetRequiredChangeType $classifier `
                -Force
            $resolved = @{}
            foreach ($entry in $entries) { $resolved[$entry.Folder] = $entry }
            $reviewed = [System.Collections.Generic.HashSet[string]]::new()
            [void]$reviewed.Add('macros')
            [void]$reviewed.Add('facade')

            # -Force retains the non-breaking 1.1.0 pin while the internal tag stays
            # breaking so cascade bookkeeping remains conservative.
            $resolved['facade'].EffectiveChangeType | Should -Be 'breaking'
            $resolved['facade'].EffectiveTargetVersion | Should -Be '1.1.0'

            $findings = @(Get-ManualSemverReviewFindings `
                -ResolvedReleaseSet $resolved `
                -WorkspaceBaseline $baseline `
                -ReviewedManualSemver $reviewed)

            $findings.Folder | Should -Contain 'facade'
            $findings.Folder | Should -Not -Contain 'app'
        }
    }

    Context 'explicit version pins' {
        It 'accepts a strictly-greater pin and derives EffectiveChangeType from the transition' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.2.3'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.3.0')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $resolved[0].EffectiveTargetVersion | Should -Be '1.3.0'
            $resolved[0].EffectiveChangeType    | Should -Be 'non-breaking'
            $resolved[0].RequestedTargetVersion | Should -Be '1.3.0'
            $resolved[0].RequestedChangeType    | Should -BeNullOrEmpty
        }

        It 'rejects a pin equal to the current version' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.2.3'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.2.3')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*already at v1.2.3*"
        }

        It 'rejects a pin lower than the current version' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.2.3'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.2.0')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*already at v1.2.3*"
        }
    }

    Context 'explicit version pin to 1.0.0' {
        It 'accepts an explicit 1.0.0 pin on a 0.x.y package' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '0.4.1'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.0.0')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $resolved[0].EffectiveTargetVersion | Should -Be '1.0.0'
            $resolved[0].EffectiveChangeType    | Should -Be 'breaking'
        }

        It 'rejects an explicit 1.0.0 pin when the package is already at 1.0.0 (pin-validation: pin must be > current)' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.0.0'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.0.0')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*'pkg'*already at v1.0.0*"
        }

        It 'rejects an explicit 1.0.0 pin when the package is already at a higher 1.x version' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.2.0'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.0.0')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*'pkg'*already at v1.2.0*"
        }
    }

    Context 'unknown / unpublished packages' {
        It 'rejects a token for a package that is not in the workspace' {
            $baseline = @((New-BaselinePackage -Folder 'real' -Version '0.1.0'))
            $parsed = Parse-ReleaseTokens -Tokens @('imaginary@patch')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*'imaginary'*not part of the workspace*"
        }

        It 'rejects a token for an unpublished package' {
            $baseline = @((New-BaselinePackage -Folder 'internal' -Version '0.1.0' -Published $false))
            $parsed = Parse-ReleaseTokens -Tokens @('internal@patch')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*'internal'*publish = false*"
        }
    }

    Context 'cargo name vs folder name lookup' {
        It 'finds a package by its underscore-normalized cargo name when the token uses hyphens' {
            $baseline = @((New-BaselinePackage -Folder 'http_extensions' -Name 'http-extensions' -Version '0.4.1'))
            $parsed = Parse-ReleaseTokens -Tokens @('http-extensions@nonbreaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $resolved.Count          | Should -Be 1
            $resolved[0].Folder      | Should -Be 'http_extensions'
            $resolved[0].Name        | Should -Be 'http-extensions'
        }
    }

    Context 'cascade to transitive dependents' {
        It 'pulls in direct & transitive published dependents as cascade-source entries' {
            $baseline = New-LinearBaseline
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $resolved.Count | Should -Be 4

            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['a'].Source | Should -Be 'user'
            $byFolder['b'].Source | Should -Be 'cascade'
            $byFolder['c'].Source | Should -Be 'cascade'
            $byFolder['d'].Source | Should -Be 'cascade'

            # Each cascade-source entry has a single reason pointing at the user target.
            $byFolder['b'].CascadeReasons.Count | Should -Be 1
            $byFolder['b'].CascadeReasons[0].Target | Should -Be 'a'
            $byFolder['c'].CascadeReasons[0].Target | Should -Be 'a'
            $byFolder['d'].CascadeReasons[0].Target | Should -Be 'a'
        }

        It 'classifies cascade dependents via cargo-semver-checks: API-broken dependent is breaking, unaffected dependent is patch' {
            # a released breaking. b's own public API broke (semver-checks:
            # breaking, e.g. it re-exports a changed type); c's did not
            # (semver-checks: none) but c must still re-release to pick up new a
            # => floored to patch.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('a') `
                    -AllowedExternalTypes @())
            )
            $classifier = New-StubClassifier @{ b = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier

            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['a'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['a'].EffectiveTargetVersion | Should -Be '2.0.0'

            $byFolder['b'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['b'].EffectiveTargetVersion | Should -Be '2.0.0'
            $byFolder['b'].CascadeReasons[0].Breaking | Should -BeTrue

            $byFolder['c'].EffectiveChangeType    | Should -Be 'patch'
            $byFolder['c'].EffectiveTargetVersion | Should -Be '1.0.1'
            $byFolder['c'].CascadeReasons[0].Breaking | Should -BeFalse
        }

        It 'derives each cascade dependent''s change type from its own semver-checks verdict, not the target''s' {
            # a -> b -> c, releasing a as patch. b's own API is non-breaking; c's
            # is unaffected. The dependent severities come from semver-checks on
            # each dependent, independent of a's (patch) change type.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') `
                    -AllowedExternalTypes @())
            )
            $classifier = New-StubClassifier @{ b = 'non-breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@patch')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveChangeType | Should -Be 'non-breaking'
            $byFolder['c'].EffectiveChangeType | Should -Be 'patch'
        }

        It 'raises an unchanged dependent when it exposes an incompatibly bumped dependency' {
            $baseline = @(
                (New-BaselinePackage -Folder 'bytesbuf' -Version '0.7.0')
                (New-BaselinePackage -Folder 'bytesbuf_io' -Version '0.7.0' -Deps @('bytesbuf') `
                    -AllowedExternalTypes @('bytesbuf::*'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('bytesbuf@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($entry in $resolved) { $byFolder[$entry.Folder] = $entry }

            $byFolder['bytesbuf_io'].EffectiveChangeType | Should -Be 'breaking'
            $byFolder['bytesbuf_io'].EffectiveTargetVersion | Should -Be '0.8.0'
            $byFolder['bytesbuf_io'].CascadeReasons[0].Breaking | Should -BeTrue
        }

        It 'keeps an unchanged dependent at patch when it does not expose the bumped dependency' {
            $baseline = @(
                (New-BaselinePackage -Folder 'dependency' -Version '1.0.0')
                (New-BaselinePackage -Folder 'dependent' -Version '1.0.0' -Deps @('dependency') `
                    -AllowedExternalTypes @('other_crate::*'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('dependency@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $dependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

            $dependent.EffectiveChangeType | Should -Be 'patch'
            $dependent.EffectiveTargetVersion | Should -Be '1.0.1'
        }

        It 'treats missing external-type metadata conservatively' {
            # $null is passed explicitly: this test is *about* absent metadata,
            # so the signal must come from the arguments and not from a helper
            # default. New-BaselinePackage defaults to @() precisely so that no
            # other test silently depends on the fail-closed branch.
            $baseline = @(
                (New-BaselinePackage -Folder 'dependency' -Version '1.0.0')
                (New-BaselinePackage -Folder 'dependent' -Version '1.0.0' -Deps @('dependency') `
                    -AllowedExternalTypes $null)
            )
            $parsed = Parse-ReleaseTokens -Tokens @('dependency@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $dependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

            $dependent.EffectiveChangeType | Should -Be 'breaking'
            $dependent.EffectiveTargetVersion | Should -Be '2.0.0'
        }

        It 'treats wildcard external-type metadata as possible exposure' {
            $baseline = @(
                (New-BaselinePackage -Folder 'dependency' -Version '1.0.0')
                (New-BaselinePackage -Folder 'dependent' -Version '1.0.0' -Deps @('dependency') `
                    -AllowedExternalTypes @('*'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('dependency@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $dependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

            $dependent.EffectiveChangeType | Should -Be 'breaking'
            $dependent.EffectiveTargetVersion | Should -Be '2.0.0'
        }

        It 'propagates exposed incompatible dependency versions through multiple direct edges' {
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0')
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a') `
                    -AllowedExternalTypes @('a::*'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') `
                    -AllowedExternalTypes @('b::*'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($entry in $resolved) { $byFolder[$entry.Folder] = $entry }

            $byFolder['b'].EffectiveChangeType | Should -Be 'breaking'
            $byFolder['c'].EffectiveChangeType | Should -Be 'breaking'
            $byFolder['c'].CascadeReasons.Target | Should -Contain 'b'
        }

        It 'cascades through an allowlist entry rooted at a renamed dependency alias' {
            # `dependent` declares `dependency` under `package = "..."` as
            # `aliased_dep`, so its allowlist can only name the alias. Matching
            # on the real package name alone would miss it and ship the break.
            $baseline = @(
                (New-BaselinePackage -Folder 'dependency' -Version '1.0.0')
                (New-BaselinePackage -Folder 'dependent' -Version '1.0.0' -Deps @('dependency') `
                    -DepAliases @{ dependency = @('aliased_dep') } `
                    -AllowedExternalTypes @('aliased_dep::Handle'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('dependency@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $dependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

            $dependent.EffectiveChangeType    | Should -Be 'breaking'
            $dependent.EffectiveTargetVersion | Should -Be '2.0.0'
        }

        It 'does not cascade when the alias in the allowlist belongs to a different dependency' {
            $baseline = @(
                (New-BaselinePackage -Folder 'dependency' -Version '1.0.0')
                (New-BaselinePackage -Folder 'dependent' -Version '1.0.0' -Deps @('dependency') `
                    -DepAliases @{ other_crate = @('aliased_dep') } `
                    -AllowedExternalTypes @('aliased_dep::Handle'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('dependency@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $dependent = $resolved | Where-Object { $_.Folder -eq 'dependent' }

            $dependent.EffectiveChangeType    | Should -Be 'patch'
            $dependent.EffectiveTargetVersion | Should -Be '1.0.1'
        }

        It 'uses a self-floor breaking verdict before cascading exposed dependency versions' {
            $baseline = @(
                (New-BaselinePackage -Folder 'dependency' -Version '1.0.0')
                (New-BaselinePackage -Folder 'dependent' -Version '1.0.0' -Deps @('dependency') `
                    -AllowedExternalTypes @('dependency::*'))
            )
            $classifier = New-StubClassifier @{ dependency = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('dependency@patch')

            $resolved = Resolve-ReleaseSet `
                -ParsedTokens $parsed `
                -WorkspaceBaseline $baseline `
                -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($entry in $resolved) { $byFolder[$entry.Folder] = $entry }

            $byFolder['dependency'].EffectiveChangeType | Should -Be 'breaking'
            $byFolder['dependent'].EffectiveChangeType | Should -Be 'breaking'
        }

        It 'adds a proc-macro-only cascade dependent at the mechanical patch floor for manual review' {
            # consumer -> macros -> implementation. Releasing implementation
            # pulls in both dependents. The proc-macro itself cannot be
            # cargo-semver-checked, while the ordinary consumer still can.
            $baseline = @(
                (New-BaselinePackage -Folder 'implementation' -Version '1.0.0')
                (New-BaselinePackage -Folder 'macros' -Version '1.0.0' -Deps @('implementation') -IsProcMacroOnly $true)
                (New-BaselinePackage -Folder 'consumer' -Version '1.0.0' -Deps @('macros'))
            )
            $calls = [System.Collections.Generic.List[string]]::new()
            $classifier = {
                param([string]$Folder, [string]$CargoName)
                $calls.Add($Folder)
                return 'none'
            }.GetNewClosure()
            $parsed = Parse-ReleaseTokens -Tokens @('implementation@patch')

            $resolved = Resolve-ReleaseSet `
                -ParsedTokens $parsed `
                -WorkspaceBaseline $baseline `
                -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($entry in $resolved) { $byFolder[$entry.Folder] = $entry }

            $byFolder['macros'].Source | Should -Be 'cascade'
            $byFolder['macros'].EffectiveChangeType | Should -Be 'patch'
            $byFolder['macros'].RequiresManualSemverReview | Should -BeTrue
            $calls | Should -Not -Contain 'macros'
            $calls | Should -Contain 'implementation'
            $calls | Should -Contain 'consumer'
        }
    }

    Context 'cascade auto-upgrade of user-source entries' {
        It 'auto-upgrades a user-source patch to non-breaking when its own semver-checks verdict requires it (and sets AutoUpgraded)' {
            # b requested as patch, but semver-checks says b's own API is
            # non-breaking, so its change type is floored up and AutoUpgraded set.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'non-breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking', 'b@patch')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].Source                   | Should -Be 'user'
            $byFolder['b'].AutoUpgraded             | Should -BeTrue
            $byFolder['b'].RequestedChangeType      | Should -Be 'patch'
            $byFolder['b'].EffectiveChangeType      | Should -Be 'non-breaking'
            $byFolder['b'].EffectiveTargetVersion   | Should -Be '1.1.0'
        }

        It 'does NOT mark AutoUpgraded when the user requested the same change type semver-checks asks for' {
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'non-breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking', 'b@nonbreaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].AutoUpgraded | Should -BeFalse
        }

        It 'does NOT downgrade the user-supplied change type when semver-checks asks for a weaker change' {
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'patch' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@patch', 'b@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['b'].EffectiveTargetVersion | Should -Be '2.0.0'
            $byFolder['b'].AutoUpgraded           | Should -BeFalse
        }
    }

    Context 'cascade interaction with explicit version pins' {
        It 'keeps the pin when it numerically satisfies the required version' {
            # a non-breaking; b's own API non-breaking (required 1.1.0). b pinned
            # to 1.5.0 (well above), so the pin is kept.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'non-breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking', 'b@1.5.0')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveTargetVersion | Should -Be '1.5.0'
            $byFolder['b'].RequestedTargetVersion | Should -Be '1.5.0'
        }

        It 'throws when the pin is numerically below the required version' {
            # b's own API broke (semver-checks: breaking) => requires 2.0.0, but
            # user pinned b at 1.1.0. Resolution must throw.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@1.1.0')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier } |
                Should -Throw -ExpectedMessage "*Cannot release 'b' as v1.1.0*requires*v2.0.0*"
        }

        It 'mentions -Force in the rejection error message so the user knows about the override' {
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@1.1.0')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier } |
                Should -Throw -ExpectedMessage '*-Force*'
        }

        It '-Force honors the explicit pin verbatim when a higher version is required' {
            # b's own API broke => normally requires 2.0.0; user pinned b at
            # 1.1.0. With -Force, b stays at 1.1.0 but the change-type tag is
            # upgraded to record the unmet requirement. Further exposure
            # propagation follows the actual 1.0.0 -> 1.1.0 transition.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@1.1.0')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier -Force -WarningAction SilentlyContinue
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveTargetVersion    | Should -Be '1.1.0'
            $byFolder['b'].RequestedTargetVersion    | Should -Be '1.1.0'
            $byFolder['b'].EffectiveChangeType       | Should -Be 'breaking'
            $byFolder['b'].PinHonoredAgainstCascade  | Should -BeTrue
        }

        It '-Force does not stop the exposure cascade at the pinned crate' {
            # Exposed chain a -> b -> c. a@breaking drives b breaking, but b is
            # pinned to 1.1.0 -- numerically compatible from 1.0.0. The pin
            # changes b's version, not b's API: b@1.1.0 is still built against
            # a@2.0.0 and exposes `a::*`, so its public API names different
            # types than b@1.0.0 did, and a consumer of `b = "1.0"` upgrades
            # into that silently. c must inherit the break even though b's own
            # version transition looks compatible.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0')
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a') `
                    -AllowedExternalTypes @('a::*'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') `
                    -AllowedExternalTypes @('b::*'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@1.1.0')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -Force -WarningAction SilentlyContinue
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            # The pin is still honored verbatim -- -Force writes the number the
            # user asked for, and only the propagation decision changes.
            $byFolder['b'].EffectiveTargetVersion   | Should -Be '1.1.0'
            $byFolder['b'].EffectiveChangeType      | Should -Be 'breaking'
            $byFolder['b'].PinHonoredAgainstCascade | Should -BeTrue

            $byFolder['c'].EffectiveChangeType      | Should -Be 'breaking'
            $byFolder['c'].EffectiveTargetVersion   | Should -Be '2.0.0'
        }

        It 'resumes the exposure cascade past a pinned crate when the pin is itself breaking' {
            # Same chain, but b is pinned to 2.0.0 -- an incompatible
            # transition -- so c must inherit the break.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0')
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a') `
                    -AllowedExternalTypes @('a::*'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') `
                    -AllowedExternalTypes @('b::*'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@2.0.0')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -Force -WarningAction SilentlyContinue
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['b'].EffectiveTargetVersion | Should -Be '2.0.0'
            $byFolder['c'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['c'].EffectiveTargetVersion | Should -Be '2.0.0'
        }

        It 'does not propagate from a compatible release that no pin suppressed' {
            # Boundary guard for the forced-pin clause: propagation is widened
            # only by PinHonoredAgainstCascade, not by every entry in the set.
            # a releases non-breaking, so b's own release stays compatible and
            # nothing was suppressed -- c must remain at its patch floor.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0')
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a') `
                    -AllowedExternalTypes @('a::*'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') `
                    -AllowedExternalTypes @('b::*'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -WarningAction SilentlyContinue
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['b'].PinHonoredAgainstCascade | Should -BeFalse
            $byFolder['b'].EffectiveChangeType      | Should -Be 'patch'
            $byFolder['c'].EffectiveChangeType      | Should -Be 'patch'
        }

        It 'does not propagate from a forced pin that suppressed only a non-breaking requirement' {
            # PinHonoredAgainstCascade is set for ANY suppressed requirement,
            # not only a breaking one. Here b's own self-check requires
            # non-breaking (1.1.0) and the user pins 1.0.1 with -Force, so the
            # flag is set while the suppressed requirement was merely additive.
            # An additive API change breaks no consumer, so c must stay at its
            # patch floor rather than being dragged to a major release.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0')
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a') `
                    -AllowedExternalTypes @('a::*'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') `
                    -AllowedExternalTypes @('b::*'))
            )
            $classifier = { param($folder) if ($folder -eq 'b') { 'non-breaking' } else { 'patch' } }
            $parsed = Parse-ReleaseTokens -Tokens @('b@1.0.1')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType $classifier -Force -WarningAction SilentlyContinue
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['b'].PinHonoredAgainstCascade | Should -BeTrue
            $byFolder['b'].EffectiveChangeType      | Should -Be 'non-breaking'
            $byFolder['b'].EffectiveTargetVersion   | Should -Be '1.0.1'

            $byFolder['c'].EffectiveChangeType      | Should -Not -Be 'breaking'
        }

        It '-Force emits a warning naming the package, the pin, the required minimum, and the sources' {
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@1.1.0')
            $warnings = @()
            $null = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier -Force -WarningVariable +warnings -WarningAction SilentlyContinue
            ($warnings -join "`n") | Should -Match '-Force'
            ($warnings -join "`n") | Should -Match "'b'"
            ($warnings -join "`n") | Should -Match 'v1\.1\.0'
            ($warnings -join "`n") | Should -Match 'v2\.0\.0'
            ($warnings -join "`n") | Should -Match 'a'
        }

        It '-Force does NOT set PinHonoredAgainstCascade when the pin already satisfies the requirement' {
            # a non-breaking; b's own API non-breaking (required 1.1.0); user
            # pinned b at 1.5.0 which already satisfies. -Force is a no-op here.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $classifier = New-StubClassifier @{ b = 'non-breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking', 'b@1.5.0')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier -Force
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].PinHonoredAgainstCascade | Should -BeFalse
            $byFolder['b'].EffectiveTargetVersion   | Should -Be '1.5.0'
        }

        It '-Force does NOT relax the always-fatal "pin not strictly greater than current" check' {
            # Pin equal to current version is always rejected, even with -Force,
            # because it would be a no-op (or downgrade) regardless of cascade.
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.2.3'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.2.3')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -Force } |
                Should -Throw -ExpectedMessage '*strictly greater than the current version*'
        }
    }

    Context 'diamond dependency with two user-source roots' {
        It 'accumulates one cascade reason per dependency into the diamond bottom and strengthens correctly' {
            # a, x are roots; c depends on both. c's own API broke (semver-checks:
            # breaking) because of x's breaking change.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'x' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('a', 'x'))
            )
            $classifier = New-StubClassifier @{ c = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@patch', 'x@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['c'].CascadeReasons.Count | Should -Be 2
            $reasonTargets = @($byFolder['c'].CascadeReasons | ForEach-Object { $_.Target } | Sort-Object)
            $reasonTargets | Should -Be @('a', 'x')

            # c's own API broke => c becomes breaking.
            $byFolder['c'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['c'].EffectiveTargetVersion | Should -Be '2.0.0'
        }
    }

    Context 'transitive cascade reason aggregation' {
        It 'records reasons for both the direct and indirect target when a middle crate is auto-upgraded' {
            # Linear chain a -> b -> c. Tokens `b a` so b iterates first, then a's
            # BFS reaches both b and c. b's own API broke (semver-checks:
            # breaking) so b is bumped to breaking; c's is unaffected so c stays
            # patch under the per-dependent classification.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') `
                    -AllowedExternalTypes @())
            )
            $classifier = New-StubClassifier @{ b = 'breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('b@patch', 'a@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier

            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['b'].EffectiveTargetVersion | Should -Be '2.0.0'
            $byFolder['b'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['b'].AutoUpgraded           | Should -BeTrue

            $byFolder['c'].EffectiveChangeType    | Should -Be 'patch'
            $byFolder['c'].EffectiveTargetVersion | Should -Be '1.0.1'

            $cReasonForB = @($byFolder['c'].CascadeReasons | Where-Object { $_.Target -eq 'b' })
            $cReasonForB.Count   | Should -Be 1
            # Breaking reflects c's own (patch) change, not b's.
            $cReasonForB[0].Breaking | Should -BeFalse

            $cReasonForA = @($byFolder['c'].CascadeReasons | Where-Object { $_.Target -eq 'a' })
            $cReasonForA.Count   | Should -Be 1
        }
    }
}

Describe 'Resolve-ReleaseSet exposure cascade over re-exported types' {
    # cargo-check-external-types attributes a re-exported type to its DEFINING
    # crate. A crate that reaches `defining::T` through an intermediate
    # therefore allowlists `defining` while depending only on the intermediate.
    # fetch_azure documents exactly this in its own manifest:
    #
    #   # azure_core re-exports its HttpClient trait from this crate;
    #   # cargo-check-external-types reports re-exports by their defining crate.
    #   "typespec_client_core::*",
    #
    # Requiring a direct dependency edge missed every such crate, which is a
    # fail-open: a breaking bump of the defining crate shipped as compatible.

    It 'raises an indirect dependent whose allowlist names the defining crate' {
        # relay is raised directly because it exposes defining. facade requires
        # the indirect defining-crate path: it depends on relay, but its
        # allowlist names defining rather than its declared dependency.
        $baseline = @(
            New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                -AllowedExternalTypes @('defining::Handle')
            New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('relay') `
                -AllowedExternalTypes @('defining::Handle')
        )
        $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

        $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
            -GetRequiredChangeType (New-StubClassifier)
        $facade = $resolved | Where-Object { $_.Folder -eq 'facade' }

        $facade.EffectiveChangeType    | Should -Be 'breaking'
        $facade.EffectiveTargetVersion | Should -Be '2.0.0'
    }

    It 'records the defining crate as the cascade reason, not the intermediate' {
        $baseline = @(
            New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                -AllowedExternalTypes @('unrelated::Thing')
            New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('relay') `
                -AllowedExternalTypes @('defining::Handle')
        )
        $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

        $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
            -GetRequiredChangeType (New-StubClassifier)
        $facade = $resolved | Where-Object { $_.Folder -eq 'facade' }

        $facade.EffectiveChangeType | Should -Be 'breaking'
        @($facade.CascadeReasons | Where-Object { $_.Target -eq 'defining' }).Count |
            Should -Be 1 -Because 'the break originates at the crate the type is defined in'
    }

    It 'raises an indirect dependent even when the intermediate stays compatible' {
        # relay explicitly claims to expose nothing, so it correctly stays at
        # its patch floor while facade above it must still break. This is the
        # unmasked case: with a direct-edge-only scan nothing reaches facade.
        $baseline = @(
            New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('relay') `
                -AllowedExternalTypes @('defining::Handle')
        )
        $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

        $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
            -GetRequiredChangeType (New-StubClassifier)

        ($resolved | Where-Object { $_.Folder -eq 'relay' }).EffectiveChangeType  | Should -Be 'patch'
        ($resolved | Where-Object { $_.Folder -eq 'facade' }).EffectiveChangeType | Should -Be 'breaking'
    }

    It "matches an indirect allowlist entry rooted at the target's [lib] name" {
        # facade names def_v1::Handle because that is what rustdoc calls the
        # type: the crate root follows defining's [lib] name, not its package
        # name. facade cannot learn this from a rename alias -- it has no edge
        # to defining to rename -- so the root has to come from the target.
        $baseline = @(
            New-BaselinePackage -Folder 'defining' -Version '1.0.0' -CrateRoot 'def_v1' `
                -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('relay') `
                -AllowedExternalTypes @('def_v1::Handle')
        )
        $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

        $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
            -GetRequiredChangeType (New-StubClassifier)

        ($resolved | Where-Object { $_.Folder -eq 'facade' }).EffectiveChangeType | Should -Be 'breaking'
    }

    Context 'a crate holding both a direct and an indirect path' {
        # The two exposure predicates accept different allowlist roots, so a
        # crate that reaches the target both ways has to be judged on both.
        # Testing only the direct edge loses the roots the indirect path earns.

        It 'raises a dependent whose renamed direct edge cannot supply the root its indirect path does' {
            # facade imports defining directly, but under `package = "..."`, so
            # on that edge the crate is nameable only as aliased_dep and the
            # direct predicate rightly refuses def_v1. facade also reaches
            # defining through relay, which re-exports its types -- and that
            # path attributes them to defining's own crate root, def_v1. The
            # allowlist entry is therefore legitimate, and facade's public API
            # really is defining's types.
            $baseline = @(
                New-BaselinePackage -Folder 'defining' -Version '1.0.0' -CrateRoot 'def_v1' `
                    -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                    -DepAliases @{ 'defining' = @('def_v1') } `
                    -AllowedExternalTypes @('def_v1::*')
                New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('defining', 'relay') `
                    -DepAliases @{ 'defining' = @('aliased_dep') } `
                    -AllowedExternalTypes @('def_v1::Handle')
            )
            $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType (New-StubClassifier)

            ($resolved | Where-Object { $_.Folder -eq 'facade' }).EffectiveChangeType |
                Should -Be 'breaking' -Because 'the indirect path earns the root the renamed direct edge cannot'
        }

        It 'does not raise a dependent whose only path to the target is the renamed direct edge' {
            # Identical to the case above except that nothing re-exports
            # defining, so lonely has no second path. Its allowlist entry
            # cannot have been earned: importing the crate as aliased_dep makes
            # def_v1::Handle unwritable, so the entry names some unrelated
            # crate that merely collides with defining's root.
            #
            # This is why the indirect test keys off a path that exists
            # independently of the direct edge rather than plain reachability:
            # reachability counts the direct edge itself and would readmit the
            # root the direct predicate just rejected.
            $baseline = @(
                New-BaselinePackage -Folder 'defining' -Version '1.0.0' -CrateRoot 'def_v1' `
                    -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'lonely' -Version '1.0.0' -Deps @('defining') `
                    -DepAliases @{ 'defining' = @('aliased_dep') } `
                    -AllowedExternalTypes @('def_v1::Handle')
            )
            $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType (New-StubClassifier)

            ($resolved | Where-Object { $_.Folder -eq 'lonely' }).EffectiveChangeType |
                Should -Not -Be 'breaking' -Because 'a renamed edge makes that root unwritable, so the entry is a collision'
        }

        It 'still requires positive evidence on the indirect path' {
            # facade holds both paths but claims to name nothing foreign, so
            # neither predicate has anything to match. Evaluating both edges
            # must not degrade into "any crate that reaches the target".
            $baseline = @(
                New-BaselinePackage -Folder 'defining' -Version '1.0.0' -CrateRoot 'def_v1' `
                    -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                    -DepAliases @{ 'defining' = @('def_v1') } `
                    -AllowedExternalTypes @('def_v1::*')
                New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('defining', 'relay') `
                    -DepAliases @{ 'defining' = @('aliased_dep') } `
                    -AllowedExternalTypes @()
            )
            $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType (New-StubClassifier)

            ($resolved | Where-Object { $_.Folder -eq 'facade' }).EffectiveChangeType |
                Should -Not -Be 'breaking'
        }
    }

    Context 'the indirect edge demands positive evidence' {
        # The direct edge fails closed on "no evidence" because an unknown must
        # not ship a break as compatible. Carrying that rule to indirect edges
        # would force every transitive dependent that lacks metadata to
        # breaking, which is a large and wrong over-cascade. These pin the
        # narrower rule.

        It 'does not raise an indirect dependent that declares no allowlist' {
            # relay's empty allowlist is a positive claim that it exposes
            # nothing, so no type of defining's can reach facade through it.
            # facade's absent metadata is not evidence to the contrary.
            $baseline = @(
                New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                    -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('relay') `
                    -AllowedExternalTypes $null
            )
            $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType (New-StubClassifier)
            $facade = $resolved | Where-Object { $_.Folder -eq 'facade' }

            $facade.EffectiveChangeType    | Should -Be 'patch'
            $facade.EffectiveTargetVersion | Should -Be '1.0.1'
        }

        It 'still fails closed for a DIRECT dependent that declares no allowlist' {
            # Same absent metadata, direct edge: must fail closed.
            $baseline = @(
                New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                    -AllowedExternalTypes $null
            )
            $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType (New-StubClassifier)

            ($resolved | Where-Object { $_.Folder -eq 'relay' }).EffectiveChangeType | Should -Be 'breaking'
        }

        It 'does not raise an indirect dependent whose allowlist names only the intermediate' {
            # facade names relay, not defining. relay itself does not expose
            # defining, so facade cannot be holding one of defining's types.
            $baseline = @(
                New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                    -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('relay') `
                    -AllowedExternalTypes @('relay::Adapter')
            )
            $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType (New-StubClassifier)

            ($resolved | Where-Object { $_.Folder -eq 'facade' }).EffectiveChangeType | Should -Be 'patch'
        }

        It 'does not raise a crate that names the target but cannot reach it' {
            # No dependency path at all: an allowlist entry naming a same-named
            # crate from elsewhere must not manufacture a cascade edge.
            $baseline = @(
                New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
                New-BaselinePackage -Folder 'stranger' -Version '1.0.0' `
                    -AllowedExternalTypes @('defining::Handle')
            )
            $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
                -GetRequiredChangeType (New-StubClassifier)

            @($resolved | Where-Object { $_.Folder -eq 'stranger' }).Count |
                Should -Be 0 -Because 'stranger is not a dependent of defining at all'
        }
    }

    It 'reaches an indirect dependent through an unpublished conduit' {
        # Unpublished crates are not released themselves but still carry types
        # between published ones, so they must not break the reachability walk.
        $baseline = @(
            New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'internal' -Version '1.0.0' -Deps @('defining') `
                -Published $false -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'facade' -Version '1.0.0' -Deps @('internal') `
                -AllowedExternalTypes @('defining::Handle')
        )
        $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

        $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
            -GetRequiredChangeType (New-StubClassifier)

        ($resolved | Where-Object { $_.Folder -eq 'facade' }).EffectiveChangeType | Should -Be 'breaking'
    }

    It 'excludes a proc-macro-only crate from the indirect edge' {
        $baseline = @(
            New-BaselinePackage -Folder 'defining' -Version '1.0.0' -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'relay' -Version '1.0.0' -Deps @('defining') `
                -AllowedExternalTypes @()
            New-BaselinePackage -Folder 'macros' -Version '1.0.0' -Deps @('relay') `
                -IsProcMacroOnly $true -AllowedExternalTypes @('defining::Handle')
        )
        $parsed = Parse-ReleaseTokens -Tokens @('defining@breaking')

        $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline `
            -GetRequiredChangeType (New-StubClassifier)
        $macros = $resolved | Where-Object { $_.Folder -eq 'macros' }

        $macros.EffectiveChangeType | Should -Be 'patch' -Because 'a proc-macro crate has no rustdoc API to expose types through'
    }
}
