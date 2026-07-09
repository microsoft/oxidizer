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

    It 'maps a missing registry baseline to none (new/unpublished crate)' {
        $out = "error: failed to retrieve crate data from registry`n`nCaused by:`n    crate foo version 999.999.999 not found in registry"
        ConvertFrom-SemverChecksOutput -Output $out | Should -Be 'none'
    }

    It 'maps "no released versions" to none' {
        ConvertFrom-SemverChecksOutput -Output 'error: foo has no released versions on the registry' | Should -Be 'none'
    }

    It 'throws (does NOT return none) on a transient registry-retrieval failure with no "not found" cause' {
        # A network/registry outage emits the generic wrapper line but no
        # crate-absent cause. Treating it as an unpublished crate ('none') would
        # silently skip classification, violating the no-fallback contract.
        $out = "error: failed to retrieve crate data from registry`n`nCaused by:`n    error sending request for url (https://index.crates.io/...): operation timed out"
        { ConvertFrom-SemverChecksOutput -Output $out -PackageName 'foo' } |
            Should -Throw -ExpectedMessage "*did not produce a parseable result for 'foo'*"
    }

    It 'throws on unrecognized output (no silent fallback)' {
        { ConvertFrom-SemverChecksOutput -Output 'some unexpected tooling error' -PackageName 'foo' } |
            Should -Throw -ExpectedMessage "*did not produce a parseable result for 'foo'*"
    }
}

Describe 'ConvertFrom-CargoInfoOutput' {
    It 'extracts the version from a cargo info block' {
        $out = "bytesbuf_io`n    An I/O adapter`nversion: 0.6.0`nlicense: MIT"
        ConvertFrom-CargoInfoOutput -Output $out | Should -Be '0.6.0'
    }

    It 'ignores a trailing yanked/annotation note after the version' {
        $out = "version: 1.2.3 (yanked)"
        ConvertFrom-CargoInfoOutput -Output $out | Should -Be '1.2.3'
    }

    It 'parses ANSI-colorized output (cargo forces colour in CI)' {
        # cargo wraps the label in SGR escapes even when piped in CI:
        # ESC[1mESC[92mversion:ESC[0m 0.6.0
        $esc = [char]0x1b
        $out = "${esc}[1m${esc}[92mversion:${esc}[0m 0.6.0"
        ConvertFrom-CargoInfoOutput -Output $out | Should -Be '0.6.0'
    }

    It 'returns $null when the crate is not published (no version line)' {
        $out = "error: crate oxidizer_nope not found in registry"
        ConvertFrom-CargoInfoOutput -Output $out | Should -BeNullOrEmpty
    }

    It 'returns $null for empty output' {
        ConvertFrom-CargoInfoOutput -Output '' | Should -BeNullOrEmpty
    }

    It 'reports the last PUBLISHED version, not an unpublished intermediate (documented limitation)' {
        # Baseline discovery reads whatever `cargo info` returns from the
        # registry: the last *published* version. If a breaking change was
        # committed as 4.0.0 but never published (an aborted release), the
        # registry still reports 3.3.3, so semver-checks diffs the working tree
        # against 3.3.3 and the delta from the unpublished 4.0.0 cannot be
        # isolated. This is intentional; the workaround is a manual explicit
        # version pin. See docs/releasing.md.
        $registryReportsLastPublished = "version: 3.3.3"
        ConvertFrom-CargoInfoOutput -Output $registryReportsLastPublished | Should -Be '3.3.3'
    }
}

Describe 'Test-CargoInfoCrateMissing' {
    It 'matches "could not find <crate> in registry"' {
        Test-CargoInfoCrateMissing -Output 'error: could not find `foo` in registry `crates-io`' | Should -BeTrue
    }
    It 'matches "not found in registry"' {
        Test-CargoInfoCrateMissing -Output 'error: crate foo not found in the registry' | Should -BeTrue
    }
    It 'matches "no matching versions"' {
        Test-CargoInfoCrateMissing -Output 'error: no matching versions for `foo`' | Should -BeTrue
    }
    It 'does NOT match a transient/config failure (so it will throw, not skip)' {
        # A network timeout or reserved-name rejection must not be mistaken for
        # an unpublished crate — that would silently skip the semver floor.
        Test-CargoInfoCrateMissing -Output 'error: failed to query registry: operation timed out' | Should -BeFalse
        Test-CargoInfoCrateMissing -Output 'error: crates-io is replaced with remote registry Foo' | Should -BeFalse
    }
    It 'does NOT match empty output' {
        Test-CargoInfoCrateMissing -Output '' | Should -BeFalse
    }
}

Describe 'Get-CargoInfoReplacementRegistry' {
    It 'extracts the replacement registry name from a source-replacement error' {
        Get-CargoInfoReplacementRegistry -Output 'error: crates-io is replaced with remote registry OxidizerDependencies;' |
            Should -Be 'OxidizerDependencies'
    }
    It 'handles the "remote registry" phrasing without a trailing punctuation' {
        Get-CargoInfoReplacementRegistry -Output 'crates-io is replaced with remote registry MyMirror' |
            Should -Be 'MyMirror'
    }
    It 'returns $null when there is no replacement message' {
        Get-CargoInfoReplacementRegistry -Output 'version: 1.2.3' | Should -BeNullOrEmpty
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
