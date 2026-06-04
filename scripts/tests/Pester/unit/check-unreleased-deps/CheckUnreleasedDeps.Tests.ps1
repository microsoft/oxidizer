# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Unit tests for scripts/lib/check-unreleased-deps.ps1 (the helpers behind
# scripts/check-unreleased-dependencies.ps1).
#
# Layout:
#   - Pure helpers (Set-StepOutput, Format-DependencyChain,
#     Format-UnreleasedDependenciesReport) are tested in isolation.
#   - Format-ReleaseEntry needs a real workspace because it does its own
#     version lookups; it's exercised against a Linear3 synthetic workspace.
#   - Invoke-CheckUnreleasedDependencies is tested against a Linear2 synthetic
#     workspace with Get-RepoRoot mocked, exercising the four behavior
#     branches (bad base ref, no findings, findings, catch path).
#
# Pester's Mock works on functions defined in the dot-sourced library because
# both end up in the same script scope.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\check-unreleased-deps.ps1')
    . (Join-Path $env:OXI_TEST_COMMON 'New-SyntheticWorkspace.ps1')
}

# ---------------------------------------------------------------------------
# Set-StepOutput
# ---------------------------------------------------------------------------

Describe 'Set-StepOutput' {

    BeforeEach {
        $script:OriginalGithubOutput = $env:GITHUB_OUTPUT
    }

    AfterEach {
        # Restore so test isolation isn't compromised even if a test forgets.
        $env:GITHUB_OUTPUT = $script:OriginalGithubOutput
    }

    It 'is a no-op when $env:GITHUB_OUTPUT is unset' {
        $env:GITHUB_OUTPUT = ''
        # No exception, no side effect to assert beyond "did not throw".
        { Set-StepOutput -Name 'foo' -Value 'bar' } | Should -Not -Throw
    }

    It 'appends "<name>=<value>" to the GITHUB_OUTPUT file when it is set' {
        $outFile = Join-Path $TestDrive 'gh_output_simple.txt'
        $env:GITHUB_OUTPUT = $outFile

        Set-StepOutput -Name 'has_findings' -Value 'true'

        Test-Path $outFile | Should -BeTrue
        $content = Get-Content -LiteralPath $outFile -Raw
        $content.TrimEnd("`r", "`n") | Should -Be 'has_findings=true'
    }

    It 'appends successive calls cumulatively (one line per call)' {
        $outFile = Join-Path $TestDrive 'gh_output_multi.txt'
        $env:GITHUB_OUTPUT = $outFile

        Set-StepOutput -Name 'k1' -Value 'v1'
        Set-StepOutput -Name 'k2' -Value 'v2'
        Set-StepOutput -Name 'k3' -Value 'v3'

        $lines = Get-Content -LiteralPath $outFile
        $lines.Count | Should -Be 3
        $lines[0] | Should -Be 'k1=v1'
        $lines[1] | Should -Be 'k2=v2'
        $lines[2] | Should -Be 'k3=v3'
    }
}

# ---------------------------------------------------------------------------
# Format-DependencyChain (pure)
# ---------------------------------------------------------------------------

Describe 'Format-DependencyChain' {

    It 'returns the single element verbatim for a length-1 chain' {
        Format-DependencyChain -Chain @('only') | Should -Be 'only'
    }

    It 'joins a length-2 chain with " -> "' {
        Format-DependencyChain -Chain @('a', 'b') | Should -Be 'a -> b'
    }

    It 'joins a length-N chain with " -> " in order' {
        Format-DependencyChain -Chain @('a', 'b', 'c', 'd') | Should -Be 'a -> b -> c -> d'
    }
}

# ---------------------------------------------------------------------------
# Format-ReleaseEntry (uses a real synthetic workspace because it does its own
# version lookups against on-disk Cargo.toml + git refs)
# ---------------------------------------------------------------------------

Describe 'Format-ReleaseEntry' {

    BeforeAll {
        # Linear3: a depends on b, b depends on c. Initial versions a=0.1.0,
        # b=0.2.0, c=0.3.0. The baseline commit is in place at HEAD already.
        Reset-ReleaseScriptCaches
        $script:Ws = New-SyntheticWorkspace -Preset Linear3 -Path (Join-Path $TestDrive 'release-entry')
    }

    BeforeEach {
        Reset-ReleaseScriptCaches
    }

    It 'renders "  - `<folder>` <base> -> <current>" when the on-disk version differs from BaseRef' {
        # Bump b's version to 0.3.0 on disk (without committing) so the current
        # version differs from HEAD.
        $script:Ws.SetVersion('b', '0.3.0')

        $out = Format-ReleaseEntry -RepoRoot $script:Ws.Path -BaseRef 'HEAD' -Folder 'b'
        $out | Should -Be '  - `b` 0.2.0 -> 0.3.0'
    }

    It 'renders "  - `<folder>` <version> (new package)" when the package does not exist at BaseRef' {
        # New package created in working tree, no commit; HEAD does not see it.
        $newPath = Join-Path $script:Ws.Path 'crates\brandnew'
        New-Item -ItemType Directory -Path $newPath -Force | Out-Null
        $newCargo = Join-Path $newPath 'Cargo.toml'
        Set-Content -LiteralPath $newCargo -NoNewline -Value @'
[package]
name = "brandnew"
version = "0.9.0"
edition = "2021"
'@

        $out = Format-ReleaseEntry -RepoRoot $script:Ws.Path -BaseRef 'HEAD' -Folder 'brandnew'
        $out | Should -Be '  - `brandnew` 0.9.0 (new package)'

        # Clean up so the next It block doesn't see this package in workspace metadata.
        Remove-Item -Recurse -Force -Path $newPath
        Reset-ReleaseScriptCaches
    }

    It 'renders "  - `<folder>` <base> -> ?" when the on-disk Cargo.toml is missing' {
        # Delete a's Cargo.toml on disk (still tracked in HEAD).
        $aCargo = Join-Path $script:Ws.Path 'crates\a\Cargo.toml'
        Remove-Item -LiteralPath $aCargo

        $out = Format-ReleaseEntry -RepoRoot $script:Ws.Path -BaseRef 'HEAD' -Folder 'a'
        $out | Should -Be '  - `a` 0.1.0 -> ?'

        # Restore so subsequent tests in this Describe see a healthy workspace.
        & git -C $script:Ws.Path checkout -- crates/a/Cargo.toml | Out-Null
        Reset-ReleaseScriptCaches
    }
}

# ---------------------------------------------------------------------------
# Format-UnreleasedDependenciesReport (pure markdown formatter)
# ---------------------------------------------------------------------------

Describe 'Format-UnreleasedDependenciesReport' {

    BeforeAll {
        function script:NewFinding {
            param(
                [string]$Folder,
                [int]$ChangedFileCount = 1,
                # DependencyChains is an array-of-string-arrays; each inner
                # array is one chain.
                [object[]]$DependencyChains = @(, @('start', 'mid', 'end'))
            )
            return [pscustomobject]@{
                Folder           = $Folder
                ChangedFileCount = $ChangedFileCount
                DependencyChains = $DependencyChains
            }
        }
    }

    It 'always emits the standard header, release-set block, and "What this means" footer' {
        $entries = @(
            '  - `pkg1` 0.1.0 -> 0.2.0',
            '  - `pkg2` 0.5.0 -> 0.6.0'
        )

        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    $entries `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @()

        $md | Should -Match '## 📦 Unreleased Workspace Dependency Changes'
        $md | Should -Match 'This PR releases the following workspace packages:'
        $md | Should -Match '  - `pkg1` 0.1.0 -> 0.2.0'
        $md | Should -Match '  - `pkg2` 0.5.0 -> 0.6.0'
        $md | Should -Match '### What this means'
        $md | Should -Match '<sub>This is an automated informational check\. It does not fail the build\.</sub>'
        # When both bucket arrays are empty, no findings tables should appear.
        $md | Should -Not -Match 'not part of this release'
        $md | Should -Not -Match 'change type is non-breaking'
        $md | Should -Not -Match '\| Package \| Files changed'
    }

    It 'uses the singular release-set header and footer when exactly one package is released' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @()

        $md | Should -Match 'This PR releases the following workspace package:'
        $md | Should -Not -Match 'This PR releases the following workspace packages:'
        # Footer must agree in number: "the released package builds ... it will resolve ..."
        $md | Should -Match 'the released package builds against the modified version'
        $md | Should -Match 'Once published, however, it will resolve'
        $md | Should -Not -Match 'the released packages build against'
        $md | Should -Not -Match 'they will resolve'
    }

    It 'uses the plural release-set header and footer when two or more packages are released' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0', '  - `pkg2` 0.5.0 -> 0.6.0') `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @()

        $md | Should -Match 'This PR releases the following workspace packages:'
        $md | Should -Not -Match 'This PR releases the following workspace package:'
        $md | Should -Match 'the released packages build against'
        $md | Should -Match 'they will resolve'
    }

    It 'ends with exactly one trailing newline' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @()
        # Body must end with one '`n', no more, no less.
        $md.EndsWith("`n") | Should -BeTrue
        $md.EndsWith("`n`n") | Should -BeFalse
    }

    It 'emits only the "not part of this release" table when only NotReleasedFindings is non-empty' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @((NewFinding -Folder 'dep1' -ChangedFileCount 3 -DependencyChains @(, @('pkg1', 'dep1')))) `
            -ElevationCandidates  @()

        $md | Should -Match 'unreleased modifications'
        $md | Should -Match '\*not\* part of this release'
        $md | Should -Match '\| `dep1` \| 3 \| `pkg1 -> dep1` \|'
        $md | Should -Not -Match 'change type is non-breaking'
    }

    It 'uses singular intro for the NotReleasedFindings table when exactly one finding is listed' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @((NewFinding -Folder 'dep1' -DependencyChains @(, @('pkg1', 'dep1')))) `
            -ElevationCandidates  @()

        $md | Should -Match 'The following workspace package has \*\*unreleased modifications\*\* \(changes newer than its last `version =` or `publish =` change\) and is \*not\* part of this release:'
        $md | Should -Not -Match 'The following workspace packages have \*\*unreleased modifications\*\*'
    }

    It 'uses plural intro for the NotReleasedFindings table when two or more findings are listed' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @(
                (NewFinding -Folder 'dep1' -DependencyChains @(, @('pkg1', 'dep1'))),
                (NewFinding -Folder 'dep2' -DependencyChains @(, @('pkg1', 'dep2')))
            ) `
            -ElevationCandidates  @()

        $md | Should -Match 'The following workspace packages have \*\*unreleased modifications\*\* \(changes newer than their last `version =` or `publish =` change\) and are \*not\* part of this release:'
        $md | Should -Not -Match 'The following workspace package has \*\*unreleased modifications\*\*'
    }

    It 'emits only the "elevation candidates" table when only ElevationCandidates is non-empty' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.1.1') `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @((NewFinding -Folder 'pkg1' -ChangedFileCount 7 -DependencyChains @(, @('pkg1'))))

        $md | Should -Match 'change type is non-breaking / patch'
        $md | Should -Match '\| `pkg1` \| 7 \| `pkg1` \|'
        $md | Should -Not -Match 'are \*not\* part of this release'
    }

    It 'uses singular intro for the ElevationCandidates table when exactly one candidate is listed' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.1.1') `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @((NewFinding -Folder 'pkg1' -DependencyChains @(, @('pkg1'))))

        $md | Should -Match 'The following workspace package \*\*is\*\* part of this release, but its change type is non-breaking / patch while it also contains modifications from earlier commits\.'
        $md | Should -Not -Match 'The following workspace packages \*\*are\*\* part of this release'
    }

    It 'uses plural intro for the ElevationCandidates table when two or more candidates are listed' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.1.1') `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @(
                (NewFinding -Folder 'pkg1' -DependencyChains @(, @('pkg1'))),
                (NewFinding -Folder 'pkg2' -DependencyChains @(, @('pkg2')))
            )

        $md | Should -Match 'The following workspace packages \*\*are\*\* part of this release, but their change type is non-breaking / patch while they also contain modifications from earlier commits\.'
        $md | Should -Not -Match 'The following workspace package \*\*is\*\* part of this release'
    }

    It 'emits BOTH tables when both buckets are non-empty' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.1.1') `
            -NotReleasedFindings  @((NewFinding -Folder 'dep_out' -DependencyChains @(, @('pkg1', 'dep_out')))) `
            -ElevationCandidates  @((NewFinding -Folder 'pkg1' -DependencyChains @(, @('pkg1'))))

        $md | Should -Match 'unreleased modifications'
        $md | Should -Match 'change type is non-breaking / patch'
        $md | Should -Match '\| `dep_out` \|'
        $md | Should -Match '\| `pkg1` \|'
    }

    It 'deduplicates dependency chains and joins them with <br>' {
        # Three chains, two of which are identical. Should render as 2 unique
        # entries joined with '<br>'.
        $finding = NewFinding -Folder 'dep1' -DependencyChains @(
            @('pkg1', 'dep1'),
            @('pkg2', 'dep1'),
            @('pkg1', 'dep1')     # duplicate of first
        )
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @($finding) `
            -ElevationCandidates  @()

        # After Sort-Object -Unique the order is 'pkg1 -> dep1', 'pkg2 -> dep1'.
        $md | Should -Match '\| `dep1` \| 1 \| `pkg1 -> dep1`<br>`pkg2 -> dep1` \|'
    }

    It 'emits multiple finding rows in the same table when the bucket has multiple findings' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @(
                (NewFinding -Folder 'depA' -ChangedFileCount 2 -DependencyChains @(, @('pkg1', 'depA'))),
                (NewFinding -Folder 'depB' -ChangedFileCount 5 -DependencyChains @(, @('pkg1', 'depB')))
            ) `
            -ElevationCandidates  @()

        $md | Should -Match '\| `depA` \| 2 \| `pkg1 -> depA` \|'
        $md | Should -Match '\| `depB` \| 5 \| `pkg1 -> depB` \|'
    }

    It 'uses LF line endings (no CRLF) for cross-platform stability' {
        $md = Format-UnreleasedDependenciesReport `
            -ReleaseEntryLines    @('  - `pkg1` 0.1.0 -> 0.2.0') `
            -NotReleasedFindings  @() `
            -ElevationCandidates  @()
        # No '\r' anywhere in the rendered body.
        $md | Should -Not -Match "`r"
    }
}

# ---------------------------------------------------------------------------
# Invoke-CheckUnreleasedDependencies (full flow against a synthetic workspace)
# ---------------------------------------------------------------------------

Describe 'Invoke-CheckUnreleasedDependencies' {

    BeforeEach {
        $script:OriginalGithubOutput = $env:GITHUB_OUTPUT
        $script:GhOut = Join-Path $TestDrive ("gh_out_" + [guid]::NewGuid().ToString('N') + '.txt')
        $env:GITHUB_OUTPUT = $script:GhOut

        $script:OutFile = Join-Path $TestDrive ("comment_" + [guid]::NewGuid().ToString('N') + '.md')

        Reset-ReleaseScriptCaches
    }

    AfterEach {
        $env:GITHUB_OUTPUT = $script:OriginalGithubOutput
    }

    Context 'when the base ref cannot be resolved' {

        It 'emits a warning, sets has_findings=false, and does not write the output file' {
            $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive ("bad-base-" + [guid]::NewGuid().ToString('N')))
            Mock -CommandName Get-RepoRoot -MockWith { $ws.Path }

            Invoke-CheckUnreleasedDependencies -BaseRef 'no-such-ref' -OutputFile $script:OutFile -WarningAction SilentlyContinue

            Test-Path $script:OutFile | Should -BeFalse
            $ghContent = Get-Content -LiteralPath $script:GhOut -Raw
            $ghContent.TrimEnd("`r", "`n") | Should -Be 'has_findings=false'
        }
    }

    Context 'when there are no modified-but-unreleased dependencies' {

        It 'prints the "no modifications" message, sets has_findings=false, and writes no output file' {
            # Pristine Linear2 workspace: nothing modified, no version-number increments.
            $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive ("clean-" + [guid]::NewGuid().ToString('N')))
            Mock -CommandName Get-RepoRoot -MockWith { $ws.Path }

            # Capture host output to verify the message is emitted.
            $output = Invoke-CheckUnreleasedDependencies -BaseRef 'HEAD' -OutputFile $script:OutFile 6>&1
            ($output | Out-String) | Should -Match 'No modified-but-unreleased workspace dependencies detected'

            Test-Path $script:OutFile | Should -BeFalse
            (Get-Content -LiteralPath $script:GhOut -Raw).TrimEnd("`r", "`n") | Should -Be 'has_findings=false'
        }
    }

    Context 'when modified-but-unreleased dependencies exist' {

        It 'writes the markdown comment with the expected sections and sets has_findings=true' {
            # Linear2: downstream → upstream. Bump downstream so it enters the
            # release set; modify upstream source so it surfaces as a
            # modified-but-unreleased dependency reached via downstream.
            $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive ("findings-" + [guid]::NewGuid().ToString('N')))
            $ws.SetVersion('downstream', '0.2.0')            # enters release set
            $ws.ModifySource('upstream', "// extra change`n")
            $ws.AddCommit('Modify upstream source, bump downstream version')

            Reset-ReleaseScriptCaches
            Mock -CommandName Get-RepoRoot -MockWith { $ws.Path }

            Invoke-CheckUnreleasedDependencies -BaseRef 'HEAD~1' -OutputFile $script:OutFile

            Test-Path $script:OutFile | Should -BeTrue
            $md = Get-Content -LiteralPath $script:OutFile -Raw

            $md | Should -Match '## 📦 Unreleased Workspace Dependency Changes'
            $md | Should -Match '  - `downstream` 0.1.0 -> 0.2.0'
            $md | Should -Match 'unreleased modifications'
            # The not-released table should list upstream reached via downstream.
            $md | Should -Match '\| `upstream` \| \d+ \| `downstream -> upstream` \|'

            (Get-Content -LiteralPath $script:GhOut -Raw).TrimEnd("`r", "`n") | Should -Be 'has_findings=true'
        }
    }

    Context 'when an inner helper throws' {

        It 'swallows the exception, sets has_findings=false, and returns without propagating' {
            $ws = New-SyntheticWorkspace -Preset Linear2 -Path (Join-Path $TestDrive ("catch-" + [guid]::NewGuid().ToString('N')))
            Mock -CommandName Get-RepoRoot -MockWith { $ws.Path }
            Mock -CommandName Get-UnreleasedModifiedDependencies -MockWith { throw "synthetic failure" }

            # The function must not throw — even with the caller's
            # ErrorActionPreference at Stop (the default in tests).
            { Invoke-CheckUnreleasedDependencies -BaseRef 'HEAD' -OutputFile $script:OutFile -WarningAction SilentlyContinue } | Should -Not -Throw

            Test-Path $script:OutFile | Should -BeFalse
            (Get-Content -LiteralPath $script:GhOut -Raw).TrimEnd("`r", "`n") | Should -Be 'has_findings=false'
        }
    }
}
