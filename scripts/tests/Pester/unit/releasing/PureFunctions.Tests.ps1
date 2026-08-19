# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')
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

    It 'orders a pre-release version before the corresponding release version (SemVer 2.0)' {
        # SemVer 2.0 §11: a pre-release version has lower precedence than the
        # associated normal version.
        Compare-SemanticVersions -version1 '1.0.0-pre01' -version2 '1.0.0' | Should -Be -1
        Compare-SemanticVersions -version1 '1.0.0-rc.1' -version2 '1.0.0' | Should -Be -1
        Compare-SemanticVersions -version1 '1.0.0' -version2 '1.0.0-rc.1' | Should -Be 1
    }

    It 'orders pre-release identifiers numerically and lexically (SemVer 2.0)' {
        # SemVer 2.0 §11.4: numeric identifiers compared numerically;
        # alphanumeric identifiers compared in ASCII sort order.
        Compare-SemanticVersions -version1 '1.0.0-alpha' -version2 '1.0.0-beta'  | Should -Be -1
        Compare-SemanticVersions -version1 '1.0.0-rc.1'  -version2 '1.0.0-rc.2'  | Should -Be -1
        Compare-SemanticVersions -version1 '1.0.0-alpha.1' -version2 '1.0.0-alpha.10' | Should -Be -1
    }

    It 'ignores build metadata in ordering (SemVer 2.0)' {
        # SemVer 2.0 §10: build metadata MUST be ignored when determining
        # version precedence.
        Compare-SemanticVersions -version1 '1.0.0+a' -version2 '1.0.0+b' | Should -Be 0
        Compare-SemanticVersions -version1 '1.0.0-rc.1+a' -version2 '1.0.0-rc.1+b' | Should -Be 0
    }

    It 'throws on 1- or 2-component inputs' {
        # Lenient pad-to-three behaviour has been retired; the helpers are
        # strict SemVer 2.0 from the outside in.
        { Compare-SemanticVersions -version1 '1.2'  -version2 '1.2.0' } | Should -Throw
        { Compare-SemanticVersions -version1 '1'    -version2 '1.0.0' } | Should -Throw
    }

    It 'throws on leading-zero components' {
        # [semver] would parse '01.2.3' as '1.2.3'; the strict regex rejects it.
        { Compare-SemanticVersions -version1 '01.2.3' -version2 '1.2.3' } | Should -Throw
        { Compare-SemanticVersions -version1 '1.2.3' -version2 '1.02.3' } | Should -Throw
    }
}

Describe 'Get-NextVersion' {
    Context 'x.y.z (x >= 1)' {
        It 'breaking increments x and resets y,z' {
            Get-NextVersion -currentVersion '1.2.3' -ChangeType 'breaking' | Should -Be '2.0.0'
            Get-NextVersion -currentVersion '9.0.0' -ChangeType 'breaking' | Should -Be '10.0.0'
        }
        It 'non-breaking increments y and resets z' {
            Get-NextVersion -currentVersion '1.2.3' -ChangeType 'non-breaking' | Should -Be '1.3.0'
            Get-NextVersion -currentVersion '1.9.99' -ChangeType 'non-breaking' | Should -Be '1.10.0'
        }
        It 'patch increments z' {
            Get-NextVersion -currentVersion '1.2.3' -ChangeType 'patch' | Should -Be '1.2.4'
        }
    }

    Context '0.x.y (x >= 1) — Cargo SemVer rules' {
        It 'breaking increments the minor component and resets the patch component' {
            Get-NextVersion -currentVersion '0.1.5' -ChangeType 'breaking' | Should -Be '0.2.0'
            Get-NextVersion -currentVersion '0.9.99' -ChangeType 'breaking' | Should -Be '0.10.0'
        }
        It 'non-breaking maps to patch in Cargo''s 0.x.y rules' {
            Get-NextVersion -currentVersion '0.1.5' -ChangeType 'non-breaking' | Should -Be '0.1.6'
        }
        It 'patch increments the patch component' {
            Get-NextVersion -currentVersion '0.1.5' -ChangeType 'patch' | Should -Be '0.1.6'
        }
    }

    Context '0.0.z — every change is breaking' {
        It 'every change type increments z' {
            Get-NextVersion -currentVersion '0.0.3' -ChangeType 'breaking' | Should -Be '0.0.4'
            Get-NextVersion -currentVersion '0.0.3' -ChangeType 'non-breaking' | Should -Be '0.0.4'
            Get-NextVersion -currentVersion '0.0.3' -ChangeType 'patch' | Should -Be '0.0.4'
        }
    }

    Context 'pre-release / build metadata are dropped from the next version' {
        It 'strips pre-release suffixes' {
            Get-NextVersion -currentVersion '1.0.0-rc.1' -ChangeType 'breaking'     | Should -Be '2.0.0'
            Get-NextVersion -currentVersion '1.0.0-rc.1' -ChangeType 'non-breaking' | Should -Be '1.1.0'
            Get-NextVersion -currentVersion '1.0.0-rc.1' -ChangeType 'patch'        | Should -Be '1.0.1'
        }
        It 'strips build metadata suffixes' {
            Get-NextVersion -currentVersion '1.0.0+meta'     -ChangeType 'breaking' | Should -Be '2.0.0'
            Get-NextVersion -currentVersion '1.0.0-rc.1+abc' -ChangeType 'breaking' | Should -Be '2.0.0'
        }
    }

    Context 'rejects malformed input' {
        It 'throws on 1- or 2-component inputs' {
            { Get-NextVersion -currentVersion '1.2' -ChangeType 'patch' } | Should -Throw
            { Get-NextVersion -currentVersion '1'   -ChangeType 'patch' } | Should -Throw
        }
        It 'throws on leading-zero components' {
            { Get-NextVersion -currentVersion '01.2.3' -ChangeType 'patch' } | Should -Throw
        }
    }
}

Describe 'Get-ChangeTypeFromVersions' {
    Context 'x.y.z (x >= 1)' {
        It 'detects breaking' { Get-ChangeTypeFromVersions -oldVersion '1.2.3' -newVersion '2.0.0' | Should -Be 'breaking' }
        It 'detects non-breaking' { Get-ChangeTypeFromVersions -oldVersion '1.2.3' -newVersion '1.3.0' | Should -Be 'non-breaking' }
        It 'detects patch' { Get-ChangeTypeFromVersions -oldVersion '1.2.3' -newVersion '1.2.4' | Should -Be 'patch' }
    }
    Context '0.x.y (x >= 1)' {
        It 'detects 0.x change as breaking' { Get-ChangeTypeFromVersions -oldVersion '0.1.0' -newVersion '0.2.0' | Should -Be 'breaking' }
        It 'detects 0.x.y change as patch' { Get-ChangeTypeFromVersions -oldVersion '0.1.0' -newVersion '0.1.1' | Should -Be 'patch' }
    }
    Context '0.0.z' {
        It 'reports every change as breaking' { Get-ChangeTypeFromVersions -oldVersion '0.0.1' -newVersion '0.0.2' | Should -Be 'breaking' }
    }
    Context 'rejects malformed input' {
        It 'throws on 1- or 2-component inputs' {
            { Get-ChangeTypeFromVersions -oldVersion '1' -newVersion '1.0.1' } | Should -Throw
        }
    }
}

Describe 'Test-IsBreakingChange' {
    Context 'x.y.z (x >= 1)' {
        It 'breaking is breaking' { Test-IsBreakingChange -oldVersion '1.0.0' -ChangeType 'breaking' | Should -BeTrue }
        It 'non-breaking is not breaking' { Test-IsBreakingChange -oldVersion '1.0.0' -ChangeType 'non-breaking' | Should -BeFalse }
        It 'patch is not breaking' { Test-IsBreakingChange -oldVersion '1.0.0' -ChangeType 'patch' | Should -BeFalse }
    }
    Context '0.x.y (x >= 1)' {
        It 'breaking is breaking' { Test-IsBreakingChange -oldVersion '0.1.0' -ChangeType 'breaking' | Should -BeTrue }
        It 'non-breaking is not breaking' { Test-IsBreakingChange -oldVersion '0.1.0' -ChangeType 'non-breaking' | Should -BeFalse }
        It 'patch is not breaking' { Test-IsBreakingChange -oldVersion '0.1.0' -ChangeType 'patch' | Should -BeFalse }
    }
    Context '0.0.z' {
        It 'every change type is breaking' {
            Test-IsBreakingChange -oldVersion '0.0.1' -ChangeType 'patch' | Should -BeTrue
            Test-IsBreakingChange -oldVersion '0.0.1' -ChangeType 'non-breaking' | Should -BeTrue
            Test-IsBreakingChange -oldVersion '0.0.1' -ChangeType 'breaking' | Should -BeTrue
        }
    }
    Context 'rejects malformed input' {
        It 'throws on 1- or 2-component inputs' {
            { Test-IsBreakingChange -oldVersion '1' -ChangeType 'breaking' } | Should -Throw
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

    It 'accepts SemVer 2.0 pre-release identifiers' {
        Test-ValidVersion -version '1.2.3-alpha'      | Should -BeTrue
        Test-ValidVersion -version '1.2.3-pre01'      | Should -BeTrue
        Test-ValidVersion -version '1.2.3-rc.1'       | Should -BeTrue
        Test-ValidVersion -version '1.0.0-alpha.beta' | Should -BeTrue
    }

    It 'accepts SemVer 2.0 build metadata' {
        Test-ValidVersion -version '1.2.3+build'      | Should -BeTrue
        Test-ValidVersion -version '1.2.3+exp.sha.5'  | Should -BeTrue
        Test-ValidVersion -version '1.0.0-rc.1+meta'  | Should -BeTrue
    }

    It 'rejects short / long forms' {
        Test-ValidVersion -version '1.2'    | Should -BeFalse
        Test-ValidVersion -version '1'      | Should -BeFalse
        Test-ValidVersion -version '1.2.3.4'| Should -BeFalse
    }

    It 'rejects non-numeric components' {
        Test-ValidVersion -version '1.x.3' | Should -BeFalse
    }

    It 'rejects leading-zero numeric components (per SemVer 2.0)' {
        Test-ValidVersion -version '01.2.3' | Should -BeFalse
        Test-ValidVersion -version '1.02.3' | Should -BeFalse
        Test-ValidVersion -version '1.2.03' | Should -BeFalse
    }

    It 'rejects malformed pre-release / build suffixes' {
        Test-ValidVersion -version '1.2.3-'     | Should -BeFalse
        Test-ValidVersion -version '1.2.3+'     | Should -BeFalse
        Test-ValidVersion -version '1.2.3-01'   | Should -BeFalse  # leading zero in numeric pre-release identifier
    }
}

Describe 'Split-SemanticVersion' {
    It 'splits a plain SemVer triple' {
        $parts = Split-SemanticVersion -version '1.2.3'
        $parts.Major      | Should -Be 1
        $parts.Minor      | Should -Be 2
        $parts.Patch      | Should -Be 3
        $parts.PreRelease | Should -BeNullOrEmpty
        $parts.Build      | Should -BeNullOrEmpty
    }

    It 'splits a pre-release version' {
        $parts = Split-SemanticVersion -version '1.0.0-rc.1'
        $parts.Major      | Should -Be 1
        $parts.Minor      | Should -Be 0
        $parts.Patch      | Should -Be 0
        $parts.PreRelease | Should -Be 'rc.1'
        $parts.Build      | Should -BeNullOrEmpty
    }

    It 'splits a version with build metadata' {
        $parts = Split-SemanticVersion -version '1.0.0-beta+meta'
        $parts.PreRelease | Should -Be 'beta'
        $parts.Build      | Should -Be 'meta'
    }

    It 'throws on invalid input' {
        { Split-SemanticVersion -version '1.2'     } | Should -Throw '*Invalid SemVer*'
        { Split-SemanticVersion -version '01.2.3'  } | Should -Throw '*Invalid SemVer*'
        { Split-SemanticVersion -version 'bogus'   } | Should -Throw '*Invalid SemVer*'
    }
}

Describe 'Test-ValidPackageName' {
    It 'accepts simple alpha names' {
        Test-ValidPackageName -packageName 'foo'   | Should -BeTrue
        Test-ValidPackageName -packageName 'foo_bar' | Should -BeTrue
        Test-ValidPackageName -packageName 'foo-bar' | Should -BeTrue
    }

    It 'accepts digits inside' {
        Test-ValidPackageName -packageName 'crate1' | Should -BeTrue
        Test-ValidPackageName -packageName '1crate' | Should -BeTrue
    }

    It 'rejects empty and overly long names' {
        Test-ValidPackageName -packageName '' | Should -BeFalse
        Test-ValidPackageName -packageName ('a' * 65) | Should -BeFalse
    }

    It 'rejects edge underscores/hyphens' {
        Test-ValidPackageName -packageName '-foo' | Should -BeFalse
        Test-ValidPackageName -packageName 'foo-' | Should -BeFalse
    }

    It 'rejects whitespace and special chars' {
        Test-ValidPackageName -packageName 'foo bar' | Should -BeFalse
        Test-ValidPackageName -packageName 'foo.bar' | Should -BeFalse
        Test-ValidPackageName -packageName 'foo/bar' | Should -BeFalse
    }
}

Describe 'ConvertFrom-SemverChecksOutput' {
    It 'maps a major-check failure to breaking' {
        $out = "     Summary semver requires new major version: 3 major and 0 minor checks failed"
        ConvertFrom-SemverChecksOutput -Output $out | Should -Be 'breaking'
    }

    It 'maps a minor-only failure to non-breaking' {
        $out = "     Summary semver requires new minor version: 0 major and 2 minor checks failed"
        ConvertFrom-SemverChecksOutput -Output $out | Should -Be 'non-breaking'
    }

    It 'maps a zero-major zero-minor summary to patch' {
        $out = "     Summary 0 major and 0 minor checks failed"
        ConvertFrom-SemverChecksOutput -Output $out | Should -Be 'patch'
    }

    It 'maps "no semver update required" to patch' {
        $out = "    Checking foo v1.2.3 -> v1.2.3 (no change; assume minor)`n     Summary no semver update required"
        ConvertFrom-SemverChecksOutput -Output $out | Should -Be 'patch'
    }

    It 'throws on a tool/build failure (no silent fallback)' {
        # Under --baseline-rev the baseline is built from source, so a failure to
        # produce a verdict (e.g. the baseline crate failed to build) must surface
        # as an error rather than being silently classified. The caller (CI report)
        # turns this into an explicit "baseline unknown" row.
        $out = "     Building foo v1.2.3 (baseline)`nerror: could not compile ``foo`` (lib)"
        { ConvertFrom-SemverChecksOutput -Output $out -PackageName 'foo' } |
            Should -Throw -ExpectedMessage "*did not produce a parseable result for 'foo'*"
    }

    It 'throws on unrecognized output (no silent fallback)' {
        { ConvertFrom-SemverChecksOutput -Output 'some unexpected tooling error' -PackageName 'foo' } |
            Should -Throw -ExpectedMessage "*did not produce a parseable result for 'foo'*"
    }

    It 'includes the Windows path-length hint on build failures' -Skip:(-not $IsWindows) {
        { ConvertFrom-SemverChecksOutput -Output 'LINK : fatal error LNK1104' -PackageName 'foo' } |
            Should -Throw -ExpectedMessage '*shorten the repository path*'
    }

}

Describe 'Get-StrongerChangeType' {
    It 'returns the higher-ranked change type' {
        Get-StrongerChangeType 'patch' 'breaking'     | Should -Be 'breaking'
        Get-StrongerChangeType 'breaking' 'patch'     | Should -Be 'breaking'
        Get-StrongerChangeType 'patch' 'non-breaking' | Should -Be 'non-breaking'
        Get-StrongerChangeType 'non-breaking' 'patch' | Should -Be 'non-breaking'
    }

    It 'treats none as below patch' {
        Get-StrongerChangeType 'patch' 'none' | Should -Be 'patch'
        Get-StrongerChangeType 'none' 'patch' | Should -Be 'patch'
        Get-StrongerChangeType 'none' 'none' | Should -Be 'none'
    }

    It 'treats unknown/empty inputs as none (rank 0)' {
        Get-StrongerChangeType 'breaking' '' | Should -Be 'breaking'
        Get-StrongerChangeType $null 'patch' | Should -Be 'patch'
    }

    It 'returns the first argument on a tie' {
        Get-StrongerChangeType 'non-breaking' 'non-breaking' | Should -Be 'non-breaking'
    }
}

Describe 'Get-PackageFolderForPath' {
    It 'returns package folder for files under crates/<x>/' {
        Get-PackageFolderForPath -Path 'crates/foo/src/lib.rs' | Should -Be 'foo'
        Get-PackageFolderForPath -Path 'crates/foo/Cargo.toml' | Should -Be 'foo'
        Get-PackageFolderForPath -Path 'crates/my_crate/sub/deeper.rs' | Should -Be 'my_crate'
    }

    It 'handles Windows-style separators' {
        Get-PackageFolderForPath -Path 'crates\foo\src\lib.rs' | Should -Be 'foo'
    }

    It 'returns null for paths outside crates/' {
        Get-PackageFolderForPath -Path 'scripts/release-packages.ps1' | Should -BeNullOrEmpty
        Get-PackageFolderForPath -Path 'Cargo.toml' | Should -BeNullOrEmpty
        Get-PackageFolderForPath -Path 'README.md' | Should -BeNullOrEmpty
    }

    It 'returns null for crates/ root itself' {
        Get-PackageFolderForPath -Path 'crates' | Should -BeNullOrEmpty
        Get-PackageFolderForPath -Path 'crates/' | Should -BeNullOrEmpty
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

Describe 'Test-PackageExposesTarget' {
    BeforeAll {
        function New-Dependent {
            param($Allowed, $DepAliases = @{})
            [pscustomobject]@{ AllowedExternalTypes = $Allowed; DepAliases = $DepAliases }
        }
    }

    It 'reports exposure when an entry is rooted at the target package' {
        $dep = New-Dependent -Allowed @('bytesbuf::Bytes', 'std::io::Error')
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeTrue
    }

    It 'normalizes hyphens in the target package name to underscores' {
        # Crate `bytesbuf-io` is referred to as `bytesbuf_io` in Rust paths.
        $dep = New-Dependent -Allowed @('bytesbuf_io::Reader')
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf-io' | Should -BeTrue
    }

    It 'reports no exposure when no entry is rooted at the target package' {
        $dep = New-Dependent -Allowed @('std::io::Error', 'core::fmt::Debug')
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeFalse
    }

    It 'fails closed when the metadata is absent entirely' {
        Test-PackageExposesTarget -Dependent (New-Dependent -Allowed $null) -TargetPackageName 'bytesbuf' | Should -BeTrue
    }

    It 'fails closed on a wildcard root that could match anything' {
        foreach ($pattern in @('*', 'byte?buf::Bytes', '[bc]ytesbuf::Bytes')) {
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed @($pattern)) -TargetPackageName 'bytesbuf' |
                Should -BeTrue -Because "'$pattern' may match the target"
        }
    }

    It 'fails closed on a malformed entry rather than silently reporting no exposure' {
        # `-split` coerces anything to a string, so these do not throw: they
        # collapse to '' and match nothing, which would let a breaking
        # dependency bump ship as a compatible release.
        foreach ($bad in @($null, '', '   ', 42, @{ a = 1 })) {
            $rendered = if ($null -eq $bad) { '$null' } else { "'$bad'" }
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed @($bad)) -TargetPackageName 'bytesbuf' |
                Should -BeTrue -Because "$rendered carries no exposure information"
        }
    }

    It 'fails closed when an entry has an empty root' {
        foreach ($bad in @('::Bytes', ' ::Bytes')) {
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed @($bad)) `
                -TargetPackageName 'bytesbuf' | Should -BeTrue `
                -Because "'$bad' has no usable crate root"
        }
    }

    It 'fails closed when a malformed entry follows valid ones' {
        $dep = New-Dependent -Allowed @('std::io::Error', $null)
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeTrue
    }

    It 'reports no exposure for an empty allowlist' {
        # An explicit empty list means the crate exposes no external types at
        # all, which is information -- unlike absent metadata.
        Test-PackageExposesTarget -Dependent (New-Dependent -Allowed @()) -TargetPackageName 'bytesbuf' | Should -BeFalse
    }

    It 'reports exposure when the entry is rooted at the alias of a renamed dependency' {
        # `buf = { package = "bytesbuf", ... }`: Rust
        # source -- and therefore the allowlist -- can only name it as `buf`.
        $dep = New-Dependent -Allowed @('buf::Bytes') -DepAliases @{ bytesbuf = @('buf') }
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeTrue
    }

    It 'normalizes hyphens in a rename alias' {
        $dep = New-Dependent -Allowed @('bytes_buf::Bytes') -DepAliases @{ bytesbuf = @('bytes_buf') }
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeTrue
    }

    It 'reports exposure under any one of several aliases for the same dependency' {
        $aliases = @{ bytesbuf = @('buf_v1', 'buf_v2') }
        foreach ($root in @('buf_v1', 'buf_v2')) {
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed @("$root::Bytes") -DepAliases $aliases) `
                -TargetPackageName 'bytesbuf' | Should -BeTrue -Because "'$root' is an alias of the target"
        }
    }

    It 'does not treat an alias of a different dependency as exposure of the target' {
        $dep = New-Dependent -Allowed @('buf::Bytes') -DepAliases @{ other_crate = @('buf') }
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeFalse
    }

    It 'still matches the real package name when that dependency is also aliased elsewhere' {
        $dep = New-Dependent -Allowed @('bytesbuf::Bytes') -DepAliases @{ bytesbuf = @('buf') }
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeTrue
    }

    It 'tolerates a package record that predates the DepAliases field' {
        $dep = [pscustomobject]@{ AllowedExternalTypes = @('std::io::Error') }
        Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeFalse
    }

    Context 'bytesbuf_io -> bytesbuf (real workspace topology)' {
        # The production instance this whole cascade exists for. bytesbuf_io
        # describes itself as "Asynchronous I/O abstractions expressed via
        # `bytesbuf` types" and its public API says so literally --
        # `pub fn reserve(&self, ...) -> BytesBuf`, `pub fn contents(&self) ->
        # &BytesBuf`, plus BytesView/Memory/HasMemory in trait signatures. A
        # breaking bytesbuf release is therefore a breaking bytesbuf_io release.
        #
        # The allowlist comes from Get-BytesBufIoAllowlist (_common), which
        # holds the literal copied from crates/bytesbuf_io/Cargo.toml.
        # ExposureCascade-RealWorkspace.Tests.ps1 asserts the real manifest
        # still equals that literal exactly, so this stays honest: if the
        # manifest gains or loses an entry, that test fails rather than this
        # one quietly asserting against a stale copy.
        BeforeAll {
            $script:BytesBufIoAllowed = Get-BytesBufIoAllowlist
        }

        It 'reports exposure of bytesbuf' {
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed $script:BytesBufIoAllowed) `
                -TargetPackageName 'bytesbuf' | Should -BeTrue
        }

        It 'reports exposure of the other allowlisted roots' {
            foreach ($target in @('ohno', 'futures_core')) {
                Test-PackageExposesTarget -Dependent (New-Dependent -Allowed $script:BytesBufIoAllowed) `
                    -TargetPackageName $target | Should -BeTrue -Because "'$target' is an allowlisted root"
            }
        }

        It 'does not report exposure of a dependency that is absent from the allowlist' {
            # bytesbuf_io depends on trait-variant, but it is a macro crate that
            # never surfaces in the public API, so it is deliberately not
            # allowlisted. This is the branch that must stay $false -- if it ever
            # returns $true the cascade degenerates into "bump everything".
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed $script:BytesBufIoAllowed) `
                -TargetPackageName 'trait-variant' | Should -BeFalse
        }

        It 'is not fooled by a crate whose name merely prefixes an allowlisted root' {
            # 'bytesbuf' is a strict prefix of 'bytesbuf_io'. Root comparison is
            # exact, so an allowlist naming one must not report the other.
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed @('bytesbuf::BytesBuf')) `
                -TargetPackageName 'bytesbuf_io' | Should -BeFalse
            Test-PackageExposesTarget -Dependent (New-Dependent -Allowed @('bytesbuf_io::Read')) `
                -TargetPackageName 'bytesbuf' | Should -BeFalse
        }
    }
}
Describe 'Test-PackageAllowlistNamesTarget' {
    BeforeAll {
        function New-Dependent {
            param($Allowed, $DepAliases = @{})
            [pscustomobject]@{ AllowedExternalTypes = $Allowed; DepAliases = $DepAliases }
        }
    }

    It 'reports a match when an entry is rooted at the target package' {
        $dep = New-Dependent -Allowed @('recoverable::Recovery', 'std::io::Error')
        Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'recoverable' | Should -BeTrue
    }

    It 'normalizes hyphens in the target package name' {
        $dep = New-Dependent -Allowed @('data_privacy_core::Sensitive')
        Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'data-privacy-core' | Should -BeTrue
    }

    It "reports a match when an entry is rooted at the target's divergent crate root" {
        $dep = New-Dependent -Allowed @('buf_core::Bytes')
        Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'bytesbuf' `
            -TargetCrateRoot 'buf_core' | Should -BeTrue
    }

    It 'does not match the package name when a divergent crate root is known' {
        # `[lib] name` replaces the package name as the Rust root. Keeping both
        # would let an unrelated crate with the package-name root force a
        # spurious breaking bump on this indirect target.
        $dep = New-Dependent -Allowed @('bytesbuf::Bytes')
        Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'bytesbuf' `
            -TargetCrateRoot 'buf_core' | Should -BeFalse
    }

    It 'does not consult dependency-edge aliases for an indirect target' {
        # An indirect dependent declares no edge to the target, so production
        # can never populate this alias. Pin that the helper does not turn an
        # impossible fixture state into supported behaviour.
        $dep = New-Dependent -Allowed @('buf::Bytes') -DepAliases @{ bytesbuf = @('buf') }
        Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'bytesbuf' | Should -BeFalse
    }

    It 'reports no match when no entry is rooted at the target' {
        $dep = New-Dependent -Allowed @('std::io::Error', 'core::fmt::Debug')
        Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'recoverable' | Should -BeFalse
    }

    Context 'divergence from Test-PackageExposesTarget' {
        # These are the cases the two functions answer differently, and the
        # difference is the whole point: this predicate gates the INDIRECT
        # dependency edge, where "no evidence" must not be read as exposure.
        # Treating absent metadata as a match there would force every transitive
        # dependent in the graph to breaking.

        It 'reports no match for absent metadata, where exposure fails closed' {
            $dep = New-Dependent -Allowed $null
            Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'recoverable' | Should -BeFalse
            # Contrast: the direct-edge predicate must fail closed on the same input.
            Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'recoverable' | Should -BeTrue
        }

        It 'skips malformed entries instead of failing closed on them' {
            foreach ($bad in @($null, '', '   ', 42, @{ a = 1 })) {
                $dep = New-Dependent -Allowed @($bad)
                Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'recoverable' |
                    Should -BeFalse -Because 'a malformed entry is not positive evidence'
                Test-PackageExposesTarget -Dependent $dep -TargetPackageName 'recoverable' |
                    Should -BeTrue -Because 'the direct edge still fails closed'
            }
        }

        It 'still finds a valid entry that follows a malformed one' {
            # Skipping malformed entries must not abandon the rest of the list.
            $dep = New-Dependent -Allowed @($null, 'recoverable::Recovery')
            Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'recoverable' | Should -BeTrue
        }

        It 'treats a wildcard root as a match, unlike other unknowns' {
            # A wildcard is a deliberate declaration that can expand to the
            # target, not an absence of information.
            foreach ($pattern in @('*', 'recover?ble::Recovery', '[rs]ecoverable::Recovery')) {
                Test-PackageAllowlistNamesTarget -Dependent (New-Dependent -Allowed @($pattern)) `
                    -TargetPackageName 'recoverable' | Should -BeTrue -Because "'$pattern' may expand to the target"
            }
        }
    }

    It 'reports no match for an explicit empty allowlist' {
        Test-PackageAllowlistNamesTarget -Dependent (New-Dependent -Allowed @()) `
            -TargetPackageName 'recoverable' | Should -BeFalse
    }

    It 'tolerates a package record that predates the DepAliases field' {
        $dep = [pscustomobject]@{ AllowedExternalTypes = @('recoverable::Recovery') }
        Test-PackageAllowlistNamesTarget -Dependent $dep -TargetPackageName 'recoverable' | Should -BeTrue
    }
}
