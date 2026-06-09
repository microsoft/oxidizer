# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Unit tests for Write-Changelog. Pinpoints two paths:
#   1. The cascade-emission path — introduced when cascadeReasons changed
#      from a single hashtable to an array of objects (so a single
#      dependent package can record being pulled in by multiple released
#      dependencies in one PR).
#   2. The unreleased-section folding path — Write-Changelog folds any
#      pre-existing `## Unreleased` / `## [Unreleased]` body into the new
#      release section ahead of the auto-generated cascade + commit bullets.
#
# Invoke-Git is mocked so these tests run hermetically (no synthetic git
# repository needed). The full end-to-end behaviour (real git history,
# commit-message rendering, README regeneration) is covered by the
# scenario suite under scripts/tests/Pester/scenarios/.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
}

Describe 'Write-Changelog cascade emission' {
    BeforeEach {
        # Hermetic: no tags, no commits. The function falls straight through
        # to the cascade-emission branch because $formattedCommits.Count -eq 0
        # but $hasCascade -eq $true.
        Mock -CommandName Invoke-Git -MockWith { @() }

        # Stable date for diffability across runs.
        Mock -CommandName Get-Date -MockWith {
            [datetime]'2026-06-15T00:00:00Z'
        }

        $script:ChangelogPath = Join-Path $TestDrive ("write-changelog-" + [guid]::NewGuid().Guid.Substring(0,8) + ".md")
        # Pre-seed a minimal CHANGELOG so the "# Changelog" header-anchored insert
        # path is exercised (the same path real packages take).
        Set-Content -LiteralPath $script:ChangelogPath -Value "# Changelog`n`n" -NoNewline -Encoding utf8
    }

    Context 'single-reason regression (the prior single-hashtable shape)' {
        It 'emits a Maintenance section with one bullet for a non-breaking cascade' {
            Write-Changelog -packageName 'pkg' -newVersion '0.2.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @(@{ Target = 'depA'; Version = '0.3.0'; Breaking = $false })

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw
            $content | Should -Match '## \[0\.2\.0\] - 2026-06-15'
            $content | Should -Match '- 🔧 Maintenance'
            $content | Should -Not -Match '- ⚠️ Breaking'
            $content | Should -Match 'Now requires `0\.3\.0` of `depA`'
            ([regex]::Matches($content, 'Now requires `')).Count | Should -Be 1
        }

        It 'emits a Breaking section with one bullet for a breaking cascade' {
            Write-Changelog -packageName 'pkg' -newVersion '1.0.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @(@{ Target = 'bigdep'; Version = '2.0.0'; Breaking = $true })

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw
            $content | Should -Match '- ⚠️ Breaking'
            $content | Should -Not -Match '- 🔧 Maintenance'
            $content | Should -Match 'Now requires `2\.0\.0` of `bigdep`'
        }
    }

    Context 'multi-reason emission' {
        It 'emits one Maintenance section with multiple bullets when no reason is breaking' {
            # Targets supplied in non-alphabetic order so the sort step is exercised.
            Write-Changelog -packageName 'pkg' -newVersion '0.5.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @(
                    @{ Target = 'zeta';  Version = '0.9.0'; Breaking = $false }
                    @{ Target = 'alpha'; Version = '0.4.0'; Breaking = $false }
                    @{ Target = 'mid';   Version = '0.7.0'; Breaking = $false }
                )

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw
            $content | Should -Match '- 🔧 Maintenance'
            $content | Should -Not -Match '- ⚠️ Breaking'

            $alphaIdx = $content.IndexOf('Now requires `0.4.0` of `alpha`')
            $midIdx   = $content.IndexOf('Now requires `0.7.0` of `mid`')
            $zetaIdx  = $content.IndexOf('Now requires `0.9.0` of `zeta`')

            $alphaIdx | Should -BeGreaterThan -1
            $midIdx   | Should -BeGreaterThan -1
            $zetaIdx  | Should -BeGreaterThan -1
            $alphaIdx | Should -BeLessThan $midIdx -Because 'bullets are sorted alphabetically by Target'
            $midIdx   | Should -BeLessThan $zetaIdx -Because 'bullets are sorted alphabetically by Target'
        }

        It 'emits a Breaking section when ANY reason is breaking, with all bullets present and sorted' {
            Write-Changelog -packageName 'pkg' -newVersion '1.1.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @(
                    @{ Target = 'safe';      Version = '0.6.0'; Breaking = $false }
                    @{ Target = 'incompat';  Version = '2.0.0'; Breaking = $true  }
                )

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw
            $content | Should -Match '- ⚠️ Breaking'
            $content | Should -Not -Match '- 🔧 Maintenance'

            $content | Should -Match 'Now requires `0\.6\.0` of `safe`'
            $content | Should -Match 'Now requires `2\.0\.0` of `incompat`'

            # Sort is alphabetical regardless of breaking flag.
            $incompatIdx = $content.IndexOf('Now requires `2.0.0` of `incompat`')
            $safeIdx     = $content.IndexOf('Now requires `0.6.0` of `safe`')
            $incompatIdx | Should -BeLessThan $safeIdx
        }

        It 'emits a Breaking section with multiple bullets when ALL reasons are breaking' {
            Write-Changelog -packageName 'pkg' -newVersion '2.0.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @(
                    @{ Target = 'b'; Version = '1.0.0'; Breaking = $true }
                    @{ Target = 'a'; Version = '3.0.0'; Breaking = $true }
                )

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw
            ([regex]::Matches($content, '- ⚠️ Breaking')).Count | Should -Be 1
            $content | Should -Not -Match '- 🔧 Maintenance'

            $aIdx = $content.IndexOf('Now requires `3.0.0` of `a`')
            $bIdx = $content.IndexOf('Now requires `1.0.0` of `b`')
            $aIdx | Should -BeLessThan $bIdx -Because 'sort is alphabetical by Target, not by Version'
        }
    }

    Context 'pscustomobject element shape' {
        It 'accepts pscustomobject reasons (the shape Resolve-ReleaseSet produces)' {
            $reasons = @(
                [pscustomobject]@{ Target = 'two'; Version = '0.2.0'; Breaking = $false }
                [pscustomobject]@{ Target = 'one'; Version = '0.1.0'; Breaking = $false }
            )

            Write-Changelog -packageName 'pkg' -newVersion '0.3.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons $reasons

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw
            $content | Should -Match 'Now requires `0\.1\.0` of `one`'
            $content | Should -Match 'Now requires `0\.2\.0` of `two`'

            $oneIdx = $content.IndexOf('Now requires `0.1.0` of `one`')
            $twoIdx = $content.IndexOf('Now requires `0.2.0` of `two`')
            $oneIdx | Should -BeLessThan $twoIdx
        }
    }

    Context 'no-cascade paths' {
        It 'emits no cascade section when reasons is null and no commits exist' {
            # No tags, no commits, no cascade reasons → warns and returns
            # WITHOUT touching the file.
            $before = Get-Content -LiteralPath $script:ChangelogPath -Raw

            Write-Changelog -packageName 'pkg' -newVersion '0.2.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -WarningAction SilentlyContinue

            $after = Get-Content -LiteralPath $script:ChangelogPath -Raw
            $after | Should -Be $before
        }

        It 'emits no cascade section when reasons is an empty array' {
            $before = Get-Content -LiteralPath $script:ChangelogPath -Raw

            Write-Changelog -packageName 'pkg' -newVersion '0.2.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @() `
                -WarningAction SilentlyContinue

            $after = Get-Content -LiteralPath $script:ChangelogPath -Raw
            $after | Should -Be $before
        }
    }

    Context 'unreleased section folding' {
        # Each test re-seeds the changelog with a different starting layout
        # before invoking Write-Changelog. The body-fold behaviour is asserted
        # via string-content checks on the resulting file.

        It 'folds a top-of-file `## Unreleased` body into the new version section and drops the orphan heading' {
            Set-Content -LiteralPath $script:ChangelogPath -Encoding utf8 -NoNewline -Value @"
# Changelog

## Unreleased

### Added
- New feature one
- New feature two

## [0.1.0] - 2024-01-01

- ✨ Features
  - earlier feature
"@

            Write-Changelog -packageName 'pkg' -newVersion '0.2.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -WarningAction SilentlyContinue

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw

            $content | Should -Match '## \[0\.2\.0\] - 2026-06-15'
            $content | Should -Match '### Added'
            $content | Should -Match 'New feature one'
            $content | Should -Match 'New feature two'
            # Earlier release section is preserved.
            $content | Should -Match '## \[0\.1\.0\] - 2024-01-01'
            # The orphan `## Unreleased` heading is gone (only the new
            # version's `## [` headings remain).
            $content | Should -Not -Match '(?im)^##[ \t]+\[?Unreleased\]?'
        }

        It 'accepts the `## [Unreleased]` bracketed header form (case-insensitive)' {
            Set-Content -LiteralPath $script:ChangelogPath -Encoding utf8 -NoNewline -Value @"
# Changelog

## [unreleased]

- ✨ Features
  - curated bullet

## [1.0.0] - 2024-01-01

initial release
"@

            Write-Changelog -packageName 'pkg' -newVersion '1.0.1' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -WarningAction SilentlyContinue

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw

            $content | Should -Match 'curated bullet'
            $content | Should -Not -Match '(?im)^##[ \t]+\[?unreleased\]?'
        }

        It 'hoists a mid-file `## Unreleased` body into the new (top) version section' {
            # Mirrors the templated_uri pattern: a previously-released version
            # sits above an orphaned Unreleased section. The fold should still
            # work — the section is removed from its mid-file position and its
            # body is folded into the new top section.
            Set-Content -LiteralPath $script:ChangelogPath -Encoding utf8 -NoNewline -Value @"
# Changelog

## [0.2.1] - 2026-05-25

- ✨ Features
  - older entry

## Unreleased

- ✨ Features
  - mid-file curated content

## [0.2.0] - 2026-05-11

- ⚠️ Breaking
"@

            Write-Changelog -packageName 'pkg' -newVersion '0.3.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -WarningAction SilentlyContinue

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw

            $content | Should -Match '## \[0\.3\.0\] - 2026-06-15'
            $content | Should -Match 'mid-file curated content'

            # New section comes first, then the previously-released entries
            # in their original order.
            $newIdx     = $content.IndexOf('## [0.3.0]')
            $v021Idx    = $content.IndexOf('## [0.2.1]')
            $v020Idx    = $content.IndexOf('## [0.2.0]')
            $newIdx | Should -BeLessThan $v021Idx
            $v021Idx | Should -BeLessThan $v020Idx

            # The orphan heading is gone.
            $content | Should -Not -Match '(?im)^##[ \t]+\[?Unreleased\]?'
        }

        It 'merges Unreleased body BEFORE cascade bullets when both are present' {
            Set-Content -LiteralPath $script:ChangelogPath -Encoding utf8 -NoNewline -Value @"
# Changelog

## Unreleased

- ✨ Features
  - manually-curated feature

## [1.0.0] - 2024-01-01

initial
"@

            Write-Changelog -packageName 'pkg' -newVersion '1.0.1' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @(@{ Target = 'bar'; Version = '2.0.0'; Breaking = $false }) `
                -WarningAction SilentlyContinue

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw

            $curatedIdx  = $content.IndexOf('manually-curated feature')
            $cascadeIdx  = $content.IndexOf('Now requires `2.0.0` of `bar`')

            $curatedIdx | Should -BeGreaterThan -1
            $cascadeIdx | Should -BeGreaterThan -1
            $curatedIdx | Should -BeLessThan $cascadeIdx -Because 'user-curated Unreleased content leads the section; auto-generated bullets follow'
        }

        It 'removes an empty `## Unreleased` section without adding empty content' {
            Set-Content -LiteralPath $script:ChangelogPath -Encoding utf8 -NoNewline -Value @"
# Changelog

## Unreleased

## [1.0.0] - 2024-01-01

initial
"@

            Write-Changelog -packageName 'pkg' -newVersion '1.0.1' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -cascadeReasons @(@{ Target = 'dep'; Version = '2.0.0'; Breaking = $false }) `
                -WarningAction SilentlyContinue

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw

            # New section emitted normally; cascade bullet present.
            $content | Should -Match '## \[1\.0\.1\] - 2026-06-15'
            $content | Should -Match 'Now requires `2\.0\.0` of `dep`'
            # Orphan Unreleased heading is gone.
            $content | Should -Not -Match '(?im)^##[ \t]+\[?Unreleased\]?'
            # No stray "## []" or empty-version artefacts.
            $content | Should -Not -Match '##[ \t]+\[\]'
        }

        It 'writes a new release section when Unreleased is the ONLY source of content (no commits, no cascade)' {
            Set-Content -LiteralPath $script:ChangelogPath -Encoding utf8 -NoNewline -Value @"
# Changelog

## Unreleased

- ✨ Features
  - just curated content
"@

            Write-Changelog -packageName 'pkg' -newVersion '0.1.0' `
                -packageFolder (Join-Path $TestDrive 'crates\pkg') `
                -changelogFile $script:ChangelogPath -prBaseUrl 'http://x' `
                -WarningAction SilentlyContinue

            $content = Get-Content -LiteralPath $script:ChangelogPath -Raw

            $content | Should -Match '## \[0\.1\.0\] - 2026-06-15'
            $content | Should -Match 'just curated content'
            $content | Should -Not -Match '(?im)^##[ \t]+\[?Unreleased\]?'
        }
    }
}
