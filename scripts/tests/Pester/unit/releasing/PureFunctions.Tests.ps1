# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\releasing.ps1')
}

Describe 'Compare-SemanticVersions' {
    It 'returns 0 for equal versions' {
        Compare-SemanticVersions -version1 '1.2.3' -version2 '1.2.3' | Should -Be 0
        Compare-SemanticVersions -version1 '0.0.1' -version2 '0.0.1' | Should -Be 0
    }

    It 'returns -1 when version1 < version2' {
        Compare-SemanticVersions -version1 '1.2.3' -version2 '1.2.4' | Should -Be -1
        Compare-SemanticVersions -version1 '1.2.3' -version2 '1.3.0' | Should -Be -1
        Compare-SemanticVersions -version1 '1.2.3' -version2 '2.0.0' | Should -Be -1
        Compare-SemanticVersions -version1 '0.1.0' -version2 '1.0.0' | Should -Be -1
    }

    It 'returns 1 when version1 > version2' {
        Compare-SemanticVersions -version1 '1.2.4' -version2 '1.2.3' | Should -Be 1
        Compare-SemanticVersions -version1 '1.3.0' -version2 '1.2.99' | Should -Be 1
        Compare-SemanticVersions -version1 '2.0.0' -version2 '1.99.99' | Should -Be 1
    }

    It 'pads short versions with zeros' {
        Compare-SemanticVersions -version1 '1.2' -version2 '1.2.0' | Should -Be 0
        Compare-SemanticVersions -version1 '1.2' -version2 '1.2.1' | Should -Be -1
        Compare-SemanticVersions -version1 '1.3' -version2 '1.2.99' | Should -Be 1
    }

    It 'pads single-segment versions with zeros (forces array context internally)' {
        # Previously, '1'.Split('.') |ForEach-Object {[int]$_} flowed a scalar out
        # of the pipeline and the pad-to-3 loop hung. Verifies the array-context
        # fix in releasing.ps1.
        Compare-SemanticVersions -version1 '1'   -version2 '1.0.0' | Should -Be 0
        Compare-SemanticVersions -version1 '1'   -version2 '1.0.1' | Should -Be -1
        Compare-SemanticVersions -version1 '2'   -version2 '1.99.99' | Should -Be 1
    }
}

Describe 'Get-NextVersion' {
    Context 'x.y.z (x >= 1)' {
        It 'major bumps x and resets y,z' {
            Get-NextVersion -currentVersion '1.2.3' -bump 'major' | Should -Be '2.0.0'
            Get-NextVersion -currentVersion '9.0.0' -bump 'major' | Should -Be '10.0.0'
        }
        It 'minor bumps y and resets z' {
            Get-NextVersion -currentVersion '1.2.3' -bump 'minor' | Should -Be '1.3.0'
            Get-NextVersion -currentVersion '1.9.99' -bump 'minor' | Should -Be '1.10.0'
        }
        It 'patch bumps z' {
            Get-NextVersion -currentVersion '1.2.3' -bump 'patch' | Should -Be '1.2.4'
        }
    }

    Context '0.x.y (x >= 1) — Cargo SemVer rules' {
        It 'major bumps x and resets y' {
            Get-NextVersion -currentVersion '0.1.5' -bump 'major' | Should -Be '0.2.0'
            Get-NextVersion -currentVersion '0.9.99' -bump 'major' | Should -Be '0.10.0'
        }
        It 'minor maps to patch in Cargo''s 0.x.y rules' {
            Get-NextVersion -currentVersion '0.1.5' -bump 'minor' | Should -Be '0.1.6'
        }
        It 'patch bumps y' {
            Get-NextVersion -currentVersion '0.1.5' -bump 'patch' | Should -Be '0.1.6'
        }
    }

    Context '0.0.z — every change is breaking' {
        It 'every bump kind bumps z' {
            Get-NextVersion -currentVersion '0.0.3' -bump 'major' | Should -Be '0.0.4'
            Get-NextVersion -currentVersion '0.0.3' -bump 'minor' | Should -Be '0.0.4'
            Get-NextVersion -currentVersion '0.0.3' -bump 'patch' | Should -Be '0.0.4'
        }
    }

    Context 'short-form inputs (pads to three segments)' {
        It 'handles two-segment inputs' {
            Get-NextVersion -currentVersion '1.2' -bump 'patch' | Should -Be '1.2.1'
            Get-NextVersion -currentVersion '0.1' -bump 'patch' | Should -Be '0.1.1'
        }
        It 'handles single-segment inputs (was a latent infinite loop)' {
            Get-NextVersion -currentVersion '1' -bump 'patch' | Should -Be '1.0.1'
            Get-NextVersion -currentVersion '2' -bump 'major' | Should -Be '3.0.0'
        }
    }
}

Describe 'Get-BumpKindFromVersions' {
    Context 'x.y.z (x >= 1)' {
        It 'detects major' { Get-BumpKindFromVersions -oldVersion '1.2.3' -newVersion '2.0.0' | Should -Be 'major' }
        It 'detects minor' { Get-BumpKindFromVersions -oldVersion '1.2.3' -newVersion '1.3.0' | Should -Be 'minor' }
        It 'detects patch' { Get-BumpKindFromVersions -oldVersion '1.2.3' -newVersion '1.2.4' | Should -Be 'patch' }
    }
    Context '0.x.y (x >= 1)' {
        It 'detects 0.x bump as major' { Get-BumpKindFromVersions -oldVersion '0.1.0' -newVersion '0.2.0' | Should -Be 'major' }
        It 'detects 0.x.y bump as patch' { Get-BumpKindFromVersions -oldVersion '0.1.0' -newVersion '0.1.1' | Should -Be 'patch' }
    }
    Context '0.0.z' {
        It 'reports every change as major' { Get-BumpKindFromVersions -oldVersion '0.0.1' -newVersion '0.0.2' | Should -Be 'major' }
    }
    Context 'short-form inputs (pads to three segments)' {
        It 'handles single-segment inputs (was a latent infinite loop)' {
            Get-BumpKindFromVersions -oldVersion '1' -newVersion '1.0.1' | Should -Be 'patch'
            Get-BumpKindFromVersions -oldVersion '1' -newVersion '2.0.0' | Should -Be 'major'
        }
    }
}

Describe 'Test-IsBreakingChange' {
    Context 'x.y.z (x >= 1)' {
        It 'major is breaking' { Test-IsBreakingChange -oldVersion '1.0.0' -bump 'major' | Should -BeTrue }
        It 'minor is not breaking' { Test-IsBreakingChange -oldVersion '1.0.0' -bump 'minor' | Should -BeFalse }
        It 'patch is not breaking' { Test-IsBreakingChange -oldVersion '1.0.0' -bump 'patch' | Should -BeFalse }
    }
    Context '0.x.y (x >= 1)' {
        It 'major is breaking' { Test-IsBreakingChange -oldVersion '0.1.0' -bump 'major' | Should -BeTrue }
        It 'minor is not breaking' { Test-IsBreakingChange -oldVersion '0.1.0' -bump 'minor' | Should -BeFalse }
        It 'patch is not breaking' { Test-IsBreakingChange -oldVersion '0.1.0' -bump 'patch' | Should -BeFalse }
    }
    Context '0.0.z' {
        It 'every bump is breaking' {
            Test-IsBreakingChange -oldVersion '0.0.1' -bump 'patch' | Should -BeTrue
            Test-IsBreakingChange -oldVersion '0.0.1' -bump 'minor' | Should -BeTrue
            Test-IsBreakingChange -oldVersion '0.0.1' -bump 'major' | Should -BeTrue
        }
    }
    Context 'short-form inputs (pads to three segments)' {
        It 'handles single-segment inputs (was a latent infinite loop)' {
            Test-IsBreakingChange -oldVersion '1' -bump 'major' | Should -BeTrue
            Test-IsBreakingChange -oldVersion '1' -bump 'minor' | Should -BeFalse
        }
    }
}

Describe 'Test-ValidVersion' {
    It 'accepts SemVer triples' {
        Test-ValidVersion -version '1.2.3' | Should -BeTrue
        Test-ValidVersion -version '0.0.0' | Should -BeTrue
        Test-ValidVersion -version '99.999.9999' | Should -BeTrue
    }

    It 'accepts empty string (optional)' {
        Test-ValidVersion -version '' | Should -BeTrue
        Test-ValidVersion -version $null | Should -BeTrue
    }

    It 'rejects pre-release and metadata suffixes' {
        Test-ValidVersion -version '1.2.3-alpha' | Should -BeFalse
        Test-ValidVersion -version '1.2.3+build' | Should -BeFalse
    }

    It 'rejects short / long forms' {
        Test-ValidVersion -version '1.2'    | Should -BeFalse
        Test-ValidVersion -version '1.2.3.4'| Should -BeFalse
    }

    It 'rejects non-numeric components' {
        Test-ValidVersion -version '1.x.3' | Should -BeFalse
    }
}

Describe 'Test-ValidCrateName' {
    It 'accepts simple alpha names' {
        Test-ValidCrateName -crateName 'foo'   | Should -BeTrue
        Test-ValidCrateName -crateName 'foo_bar' | Should -BeTrue
        Test-ValidCrateName -crateName 'foo-bar' | Should -BeTrue
    }

    It 'accepts digits inside' {
        Test-ValidCrateName -crateName 'crate1' | Should -BeTrue
        Test-ValidCrateName -crateName '1crate' | Should -BeTrue
    }

    It 'rejects empty and overly long names' {
        Test-ValidCrateName -crateName '' | Should -BeFalse
        Test-ValidCrateName -crateName ('a' * 65) | Should -BeFalse
    }

    It 'rejects edge underscores/hyphens' {
        Test-ValidCrateName -crateName '-foo' | Should -BeFalse
        Test-ValidCrateName -crateName 'foo-' | Should -BeFalse
    }

    It 'rejects whitespace and special chars' {
        Test-ValidCrateName -crateName 'foo bar' | Should -BeFalse
        Test-ValidCrateName -crateName 'foo.bar' | Should -BeFalse
        Test-ValidCrateName -crateName 'foo/bar' | Should -BeFalse
    }
}

Describe 'Test-CrateExposesTarget' {
    It 'returns true when no allowed_external_types declared (conservative default)' {
        $dep = [pscustomobject]@{ AllowedExternalTypes = $null }
        Test-CrateExposesTarget -dependent $dep -targetPackageName 'foo' | Should -BeTrue
    }

    It 'returns true when target appears as a root in allowed_external_types' {
        $dep = [pscustomobject]@{ AllowedExternalTypes = @('foo::*', 'bar::Baz') }
        Test-CrateExposesTarget -dependent $dep -targetPackageName 'foo' | Should -BeTrue
        Test-CrateExposesTarget -dependent $dep -targetPackageName 'bar' | Should -BeTrue
    }

    It 'returns false when target is not in allowed_external_types' {
        $dep = [pscustomobject]@{ AllowedExternalTypes = @('std::*') }
        Test-CrateExposesTarget -dependent $dep -targetPackageName 'foo' | Should -BeFalse
    }

    It 'normalizes hyphens to underscores when matching' {
        $dep = [pscustomobject]@{ AllowedExternalTypes = @('my_crate::*') }
        Test-CrateExposesTarget -dependent $dep -targetPackageName 'my-crate' | Should -BeTrue
    }

    It 'matches whole-root only, not prefix' {
        $dep = [pscustomobject]@{ AllowedExternalTypes = @('foobar::*') }
        Test-CrateExposesTarget -dependent $dep -targetPackageName 'foo' | Should -BeFalse
    }
}

Describe 'Get-CrateFolderForPath' {
    It 'returns crate folder for files under crates/<x>/' {
        Get-CrateFolderForPath -Path 'crates/foo/src/lib.rs' | Should -Be 'foo'
        Get-CrateFolderForPath -Path 'crates/foo/Cargo.toml' | Should -Be 'foo'
        Get-CrateFolderForPath -Path 'crates/my_crate/sub/deeper.rs' | Should -Be 'my_crate'
    }

    It 'handles Windows-style separators' {
        Get-CrateFolderForPath -Path 'crates\foo\src\lib.rs' | Should -Be 'foo'
    }

    It 'returns null for paths outside crates/' {
        Get-CrateFolderForPath -Path 'scripts/release-crate.ps1' | Should -BeNullOrEmpty
        Get-CrateFolderForPath -Path 'Cargo.toml' | Should -BeNullOrEmpty
        Get-CrateFolderForPath -Path 'README.md' | Should -BeNullOrEmpty
    }

    It 'returns null for crates/ root itself' {
        Get-CrateFolderForPath -Path 'crates' | Should -BeNullOrEmpty
        Get-CrateFolderForPath -Path 'crates/' | Should -BeNullOrEmpty
    }
}

Describe 'Sort-KeysByPreferredOrder' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    It 'places preferred keys first in declared order' {
        $r = Sort-KeysByPreferredOrder -allKeys @('z', 'a', 'name', 'version') -preferredOrder @('name', 'version')
        $r | Should -Be @('name', 'version', 'a', 'z')
    }

    It 'sorts non-preferred keys alphabetically' {
        $r = Sort-KeysByPreferredOrder -allKeys @('zeta', 'alpha', 'mu') -preferredOrder @()
        $r | Should -Be @('alpha', 'mu', 'zeta')
    }

    It 'omits preferred keys that are not in the input' {
        $r = Sort-KeysByPreferredOrder -allKeys @('a', 'b') -preferredOrder @('z', 'a')
        $r | Should -Be @('a', 'b')
    }

    It 'returns an empty result for empty input' {
        $r = Sort-KeysByPreferredOrder -allKeys @() -preferredOrder @('a', 'b')
        $r.Count | Should -Be 0
    }
}

Describe 'Format-ConventionalCommits' {
    BeforeAll {
        . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
    }

    It 'returns an empty array for no commits' {
        $r = Format-ConventionalCommits -rawCommitMessages @() -prBaseUrl 'https://github.com/o/r/pull'
        $r.Count | Should -Be 0
    }

    It 'returns an empty array for null commits' {
        $r = Format-ConventionalCommits -rawCommitMessages $null -prBaseUrl ''
        $r.Count | Should -Be 0
    }

    It 'groups feat / fix / docs into their canonical headers' {
        $msgs = @(
            'feat(foo): add splines',
            'fix(foo): correct underflow',
            'docs: update README'
        )
        $r = Format-ConventionalCommits -rawCommitMessages $msgs -prBaseUrl ''
        $joined = $r -join "`n"
        $joined | Should -Match '(?ms)Features.*splines'
        $joined | Should -Match '(?ms)Bug Fixes.*underflow'
        $joined | Should -Match '(?ms)Documentation.*README'
    }

    It 'lifts breaking-marked commits to a Breaking section' {
        $msgs = @(
            'feat!: remove deprecated API',
            'feat: minor addition'
        )
        $r = Format-ConventionalCommits -rawCommitMessages $msgs -prBaseUrl ''
        $joined = $r -join "`n"
        # 'breaking' bucket comes first per $TypeOrder
        $joined | Should -Match '(?ms)Breaking.*remove deprecated API'
        $joined | Should -Match '(?ms)Features.*minor addition'
        # Breaking section header appears before Features section header.
        $breakingIdx = $joined.IndexOf('Breaking')
        $featIdx     = $joined.IndexOf('Features')
        $breakingIdx | Should -BeLessThan $featIdx
    }

    It 'linkifies PR references when -prBaseUrl is supplied' {
        $msgs = @('feat(foo): add bar (#123)')
        $r = Format-ConventionalCommits -rawCommitMessages $msgs -prBaseUrl 'https://github.com/o/r/pull'
        ($r -join "`n") | Should -Match '\[#123\]\(https://github.com/o/r/pull/123\)'
    }

    It 'omits the PR link when -prBaseUrl is empty' {
        $msgs = @('feat(foo): add bar (#123)')
        $r = Format-ConventionalCommits -rawCommitMessages $msgs -prBaseUrl ''
        # Should still mention the PR reference text verbatim
        ($r -join "`n") | Should -Match '\(#123\)'
        ($r -join "`n") | Should -Not -Match 'pull/123'
    }

    It 'drops commits whose type is in IgnoredTypes' {
        # 'test' is the only ignored type at present.
        $msgs = @(
            'test: cover edge cases',
            'feat: kept'
        )
        $r = Format-ConventionalCommits -rawCommitMessages $msgs -prBaseUrl ''
        ($r -join "`n") | Should -Match 'kept'
        ($r -join "`n") | Should -Not -Match 'cover edge cases'
    }

    It 'preserves non-conventional commits under a miscellaneous section' {
        $msgs = @('totally unstructured commit message')
        $r = Format-ConventionalCommits -rawCommitMessages $msgs -prBaseUrl ''
        ($r -join "`n") | Should -Match 'totally unstructured commit message'
    }
}

Describe 'Reduce-DependencyChains' {
    It 'returns an empty array when given no chains' {
        $out = Reduce-DependencyChains -Chains @()
        @($out).Count | Should -Be 0
    }

    It 'keeps a single chain unchanged' {
        $out = Reduce-DependencyChains -Chains @(, @('foo', 'bar', 'baz'))
        @($out).Count | Should -Be 1
        $out[0] -join '|' | Should -Be 'foo|bar|baz'
    }

    It 'deduplicates identical chains' {
        $out = Reduce-DependencyChains -Chains @(@('a', 'b'), @('a', 'b'))
        @($out).Count | Should -Be 1
    }

    It 'drops a chain that is a strict suffix of another chain' {
        # 'bar -> baz' is fully contained as the tail of 'foo -> bar -> baz'.
        $out = Reduce-DependencyChains -Chains @(@('bar', 'baz'), @('foo', 'bar', 'baz'))
        @($out).Count | Should -Be 1
        $out[0] -join '|' | Should -Be 'foo|bar|baz'
    }

    It 'preserves multiple non-subsuming chains with different roots and intermediates' {
        $out = Reduce-DependencyChains -Chains @(
            @('foo', 'bar', 'baz'),
            @('quu', 'nuu', 'baz'),
            @('lurk', 'baz')
        )
        @($out).Count | Should -Be 3
        # Output is sorted alphabetically by joined chain text.
        ($out | ForEach-Object { $_ -join ' -> ' }) -join '|' |
            Should -Be 'foo -> bar -> baz|lurk -> baz|quu -> nuu -> baz'
    }

    It 'does NOT drop a shorter chain that is NOT a tail-aligned suffix' {
        # 'b -> c' is not a suffix of 'a -> b -> d' (last element differs).
        $out = Reduce-DependencyChains -Chains @(@('a', 'b', 'd'), @('b', 'c'))
        @($out).Count | Should -Be 2
    }

    It 'does NOT drop a shorter chain that overlaps the head, not the tail, of a longer chain' {
        # 'foo -> bar' overlaps the head of 'foo -> bar -> baz' but is not a suffix.
        $out = Reduce-DependencyChains -Chains @(@('foo', 'bar'), @('foo', 'bar', 'baz'))
        @($out).Count | Should -Be 2
    }

    It 'collapses several chains into one when all are nested suffixes' {
        $out = Reduce-DependencyChains -Chains @(
            @('d'),
            @('c', 'd'),
            @('b', 'c', 'd'),
            @('a', 'b', 'c', 'd')
        )
        @($out).Count | Should -Be 1
        $out[0] -join '|' | Should -Be 'a|b|c|d'
    }

    It 'returns chains in stable alphabetical order regardless of input order' {
        $a = Reduce-DependencyChains -Chains @(@('z', 'baz'), @('a', 'baz'))
        $b = Reduce-DependencyChains -Chains @(@('a', 'baz'), @('z', 'baz'))
        ($a | ForEach-Object { $_ -join ' -> ' }) -join '|' |
            Should -Be (($b | ForEach-Object { $_ -join ' -> ' }) -join '|')
        ($a | ForEach-Object { $_ -join ' -> ' }) -join '|' | Should -Be 'a -> baz|z -> baz'
    }
}
