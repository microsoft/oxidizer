# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Unit tests for Write-Changelog's cascade-emission path. Pinpoints the
# multi-reason behavior introduced when cascadeReasons changed from a single
# hashtable to an array of objects (so a single downstream package can record
# being pulled in by multiple released dependencies in one PR). Invoke-Git is
# mocked so these tests run hermetically (no synthetic git repository needed).
#
# Out of scope here: the merge-into-existing-section path (covered indirectly
# by the GitFs Add-CascadeBulletToVersionSection tests and the end-to-end
# scenario suite) and the version-section formatter (untouched by the refactor).

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
}
