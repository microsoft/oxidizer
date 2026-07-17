# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')

    # Helper that builds a baseline package record. Underscore-only cargo
    # names by default so the test stays focused on the cascade/resolve logic
    # rather than name normalization.
    function New-BaselinePackage {
        param(
            [string]   $Folder,
            [string]   $Name = $null,
            [string]   $Version = '0.1.0',
            [string[]] $Deps = @(),
            [bool]     $Published = $true
        )
        if ([string]::IsNullOrEmpty($Name)) { $Name = $Folder }
        return [pscustomobject]@{
            Folder    = $Folder
            Name      = $Name
            Version   = $Version
            Published = $Published
            Deps      = $Deps
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
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('a'))
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
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b'))
            )
            $classifier = New-StubClassifier @{ b = 'non-breaking' }
            $parsed = Parse-ReleaseTokens -Tokens @('a@patch')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline -GetRequiredChangeType $classifier
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveChangeType | Should -Be 'non-breaking'
            $byFolder['c'].EffectiveChangeType | Should -Be 'patch'
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
            # upgraded so any further cascade decisions for b would be correct.
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
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b'))
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
