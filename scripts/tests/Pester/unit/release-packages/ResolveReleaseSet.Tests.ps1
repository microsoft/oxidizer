# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
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
            [bool]     $Published = $true,
            $AllowedExternalTypes = $null
        )
        if ([string]::IsNullOrEmpty($Name)) { $Name = $Folder }
        return [pscustomobject]@{
            Folder               = $Folder
            Name                 = $Name
            Version              = $Version
            Published            = $Published
            Deps                 = $Deps
            AllowedExternalTypes = $AllowedExternalTypes
        }
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

    Context 'graduation @1.0.0' {
        It 'accepts graduation on a 0.x.y package and pins the target version to 1.0.0' {
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '0.4.1'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.0.0')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $resolved[0].IsGraduation           | Should -BeTrue
            $resolved[0].EffectiveTargetVersion | Should -Be '1.0.0'
            $resolved[0].EffectiveChangeType    | Should -Be 'breaking'
        }

        It 'rejects graduation when the package is already at >= 1.0.0 (caught by pin-validation)' {
            # The pin (1.0.0) equals the current (1.0.0), so the
            # already-at-version pin validator fires first. That is acceptable
            # — the message is clear enough; we just lock in the surface
            # behaviour here.
            $baseline = @((New-BaselinePackage -Folder 'pkg' -Version '1.0.0'))
            $parsed = Parse-ReleaseTokens -Tokens @('pkg@1.0.0')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*'pkg'*already at v1.0.0*"
        }

        It 'rejects graduation when the package is already at a higher 1.x version' {
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

        It 'cascades a breaking change as breaking to exposing dependents and as patch to non-exposing dependents' {
            # a (breaking), b exposes a (so cascade -> breaking), c does NOT expose a (-> patch).
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a') -AllowedExternalTypes @('a'))
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('a') -AllowedExternalTypes @())
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline

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

        It 'cascade BFS does NOT pass through cascade-source entries (one-level only)' {
            # a -> b -> c.  Releasing 'a' as patch makes b non-exposing→patch and
            # c non-exposing→patch.  Even if b *would* have been "breaking" if
            # released directly, the cascade from a only ever asks for patch
            # change types on transitive dependents (because b does not expose a's
            # types and c does not expose a's types).
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a') -AllowedExternalTypes @())
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('b') -AllowedExternalTypes @())
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@patch')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveChangeType | Should -Be 'patch'
            $byFolder['c'].EffectiveChangeType | Should -Be 'patch'
        }
    }

    Context 'cascade auto-upgrade of user-source entries' {
        It 'auto-upgrades a user-source patch to non-breaking when cascade requires it (and sets AutoUpgraded)' {
            # a -> b. Release a as non-breaking, release b as patch. Cascade from
            # a's non-breaking exposes b -> non-breaking, so b's patch request
            # gets auto-upgraded.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking', 'b@patch')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].Source                   | Should -Be 'user'
            $byFolder['b'].AutoUpgraded             | Should -BeTrue
            $byFolder['b'].RequestedChangeType      | Should -Be 'patch'
            $byFolder['b'].EffectiveChangeType      | Should -Be 'non-breaking'
            $byFolder['b'].EffectiveTargetVersion   | Should -Be '1.1.0'
        }

        It 'does NOT mark AutoUpgraded when the user requested the same change type the cascade asks for' {
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking', 'b@nonbreaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].AutoUpgraded | Should -BeFalse
        }

        It 'does NOT downgrade the user-supplied change type when cascade asks for a weaker change' {
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@patch', 'b@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['b'].EffectiveTargetVersion | Should -Be '2.0.0'
            $byFolder['b'].AutoUpgraded           | Should -BeFalse
        }
    }

    Context 'cascade interaction with explicit version pins' {
        It 'keeps the pin when it numerically satisfies the cascade requirement' {
            # a -> b. a non-breaking. b pinned to 1.5.0 (well above cascade 1.1.0 req).
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@nonbreaking', 'b@1.5.0')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }
            $byFolder['b'].EffectiveTargetVersion | Should -Be '1.5.0'
            $byFolder['b'].RequestedTargetVersion | Should -Be '1.5.0'
        }

        It 'throws when the pin is numerically below the cascade requirement' {
            # a breaking would require b 2.0.0 (cascade-required), but user pinned
            # b at 1.1.0. Resolution must throw.
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'b' -Version '1.0.0' -Deps @('a'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@breaking', 'b@1.1.0')
            { Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline } |
                Should -Throw -ExpectedMessage "*Cannot release 'b' as v1.1.0*cascade requires*v2.0.0*"
        }
    }

    Context 'diamond dependency with two user-source roots' {
        It 'accumulates one cascade reason per upstream into the diamond bottom and strengthens correctly' {
            # diamond:  a, x are roots;  both depended on by mid;  c depends on mid.
            # Actually:  c depends on a (patch) and c depends on b (breaking).
            $baseline = @(
                (New-BaselinePackage -Folder 'a' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'x' -Version '1.0.0' -Deps @())
                (New-BaselinePackage -Folder 'c' -Version '1.0.0' -Deps @('a', 'x'))
            )
            $parsed = Parse-ReleaseTokens -Tokens @('a@patch', 'x@breaking')
            $resolved = Resolve-ReleaseSet -ParsedTokens $parsed -WorkspaceBaseline $baseline
            $byFolder = @{}
            foreach ($e in $resolved) { $byFolder[$e.Folder] = $e }

            $byFolder['c'].CascadeReasons.Count | Should -Be 2
            $reasonTargets = @($byFolder['c'].CascadeReasons | ForEach-Object { $_.Target } | Sort-Object)
            $reasonTargets | Should -Be @('a', 'x')

            # x's cascade is breaking → c becomes breaking via cascade.
            $byFolder['c'].EffectiveChangeType    | Should -Be 'breaking'
            $byFolder['c'].EffectiveTargetVersion | Should -Be '2.0.0'
        }
    }
}
