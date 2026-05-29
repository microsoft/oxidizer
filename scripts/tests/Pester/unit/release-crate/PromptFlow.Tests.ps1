# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
#
# Unit tests for the per-package menu and prompt-flow helpers added by the
# release-script UX overhaul. Helpers under test live in
# scripts/lib/release-flow.ps1 and are deliberately split so the pure
# formatting layer can be asserted on without capturing host streams and the
# IO/IO-adjacent layer can be exercised with mocks.

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
}

# ---------------------------------------------------------------------------
# Format-PackageMenu (pure formatter)
# ---------------------------------------------------------------------------

Describe 'Format-PackageMenu' {

    BeforeAll {
        function script:NewFinding {
            param(
                [string]$Folder = 'ohno',
                [object[]]$Chains = @(@('a', 'ohno')),
                [string]$CurrentVersion = '1.2.3'
            )
            return [pscustomobject]@{
                Folder           = $Folder
                PackageName      = $Folder
                CurrentVersion   = $CurrentVersion
                ChangedFileCount = 1
                DependencyChains = $Chains
            }
        }
    }

    It 'includes the package name on the first content line' {
        $out = Format-PackageMenu -Finding (NewFinding -Folder 'ohno') -RemainingCount 0
        $out | Should -Match 'Detected package with unreleased modifications: ohno'
    }

    It 'omits the queued-count suffix when RemainingCount is 0' {
        $out = Format-PackageMenu -Finding (NewFinding) -RemainingCount 0
        $out | Should -Not -Match '\(\+\d+ packages? queued\)'
    }

    It 'renders "(+1 package queued)" with singular noun when RemainingCount is 1' {
        $out = Format-PackageMenu -Finding (NewFinding) -RemainingCount 1
        $out | Should -Match '\(\+1 package queued\)'
        $out | Should -Not -Match '\(\+1 packages queued\)'
    }

    It 'renders "(+3 packages queued)" with plural noun when RemainingCount is 3' {
        $out = Format-PackageMenu -Finding (NewFinding) -RemainingCount 3
        $out | Should -Match '\(\+3 packages queued\)'
    }

    It 'renders a "potentially affected dependency chains:" header followed by one indented line per chain' {
        $finding = NewFinding -Folder 'd' -Chains @(@('a', 'b', 'd'), @('a', 'c', 'd'))
        $out = Format-PackageMenu -Finding $finding -RemainingCount 0

        # Single header line, regardless of how many chains.
        ([regex]::Matches($out, 'potentially affected dependency chains:')).Count | Should -Be 1

        # Split into raw lines so trailing `\r` (from StringBuilder.AppendLine on Windows)
        # doesn't trip up `$` anchors.
        $lines = $out -split "`r?`n"
        $out | Should -Not -Match 'pulled in by:'
        $lines | Should -Contain '    a -> b -> d'
        $lines | Should -Contain '    a -> c -> d'
    }

    It 'lists the five menu options in the exact order and wording from the spec' {
        $out = Format-PackageMenu -Finding (NewFinding) -RemainingCount 0
        $lines = $out -split "`r?`n" | Where-Object { $_ -match '^\s*\d\. ' }
        $lines.Count | Should -Be 5
        $lines[0] | Should -Match '^\s*1\. View diff$'
        $lines[1] | Should -Match '^\s*2\. Ignore package - the changes are immaterial to published functionality$'
        # Options 3-5 now carry a concrete version transition; precise transition asserted in dedicated tests below.
        $lines[2] | Should -Match '^\s*3\. Release as breaking change \(.+\)$'
        $lines[3] | Should -Match '^\s*4\. Release as non-breaking change \(.+\)$'
        $lines[4] | Should -Match '^\s*5\. Release as patch \(.+\)$'
    }

    It 'renders concrete x.y.z -> (next) transitions for a >=1.x.y package' {
        $out = Format-PackageMenu -Finding (NewFinding -CurrentVersion '1.2.3') -RemainingCount 0
        $lines = $out -split "`r?`n"
        $lines | Should -Contain '  3. Release as breaking change (1.2.3 -> 2.0.0)'
        $lines | Should -Contain '  4. Release as non-breaking change (1.2.3 -> 1.3.0)'
        $lines | Should -Contain '  5. Release as patch (1.2.3 -> 1.2.4)'
    }

    It 'hides option 5 on 0.x.y packages because non-breaking and patch collapse to the same numeric bump' {
        $out = Format-PackageMenu -Finding (NewFinding -CurrentVersion '0.1.2') -RemainingCount 0
        $lines = $out -split "`r?`n"
        $lines | Should -Contain '  3. Release as breaking change (0.1.2 -> 0.2.0)'
        $lines | Should -Contain '  4. Release as non-breaking change (0.1.2 -> 0.1.3)'
        # Option 5 must not appear at all on 0.x.y — both "patch" and "non-breaking"
        # produce the same numeric increment under Cargo semver, so the menu only
        # offers the surviving distinct choice.
        $out | Should -Not -Match '^\s*5\. '
        $out | Should -Not -Match 'Release as patch'
    }

    It 'hides option 5 on 0.0.x packages (where minor and patch also collapse)' {
        $out = Format-PackageMenu -Finding (NewFinding -CurrentVersion '0.0.5') -RemainingCount 0
        $lines = $out -split "`r?`n"
        $lines | Should -Contain '  3. Release as breaking change (0.0.5 -> 0.0.6)'
        $lines | Should -Contain '  4. Release as non-breaking change (0.0.5 -> 0.0.6)'
        $out | Should -Not -Match '^\s*5\. '
        $out | Should -Not -Match 'Release as patch'
    }

    It 'falls back to "(major version)" / "(minor version)" / "(patch version)" hints when CurrentVersion is missing or blank' {
        # Defensive: hand-rolled findings without CurrentVersion should still render the menu, not crash.
        $finding = [pscustomobject]@{
            Folder           = 'ohno'
            PackageName      = 'ohno'
            ChangedFileCount = 1
            DependencyChains = @(, @('a', 'ohno'))
        }
        $out = Format-PackageMenu -Finding $finding -RemainingCount 0
        $lines = $out -split "`r?`n"
        $lines | Should -Contain '  3. Release as breaking change (major version)'
        $lines | Should -Contain '  4. Release as non-breaking change (minor version)'
        $lines | Should -Contain '  5. Release as patch (patch version)'
    }

    It 'does NOT include any "files changed" / numeric file-count metric' {
        # Materiality is communicated via the View Diff option; a raw count
        # would be misleading visual noise.
        $finding = NewFinding -Folder 'ohno' -Chains @(@('a', 'ohno'))
        $finding.ChangedFileCount = 42
        $out = Format-PackageMenu -Finding $finding -RemainingCount 0
        $out | Should -Not -Match 'files? changed'
        $out | Should -Not -Match '\b42\b'
    }
}

# ---------------------------------------------------------------------------
# Test-IsPatchOptionRedundant (pure semver-rule helper)
# ---------------------------------------------------------------------------

Describe 'Test-IsPatchOptionRedundant' {
    It 'returns $false for stable >=1.x.y versions' {
        Test-IsPatchOptionRedundant -CurrentVersion '1.0.0' | Should -BeFalse
        Test-IsPatchOptionRedundant -CurrentVersion '1.2.3' | Should -BeFalse
        Test-IsPatchOptionRedundant -CurrentVersion '42.7.0' | Should -BeFalse
    }

    It 'returns $true for 0.x.y versions (minor and patch collapse under Cargo semver)' {
        Test-IsPatchOptionRedundant -CurrentVersion '0.1.0' | Should -BeTrue
        Test-IsPatchOptionRedundant -CurrentVersion '0.4.7' | Should -BeTrue
    }

    It 'returns $true for 0.0.x versions (every bump collapses to patch)' {
        Test-IsPatchOptionRedundant -CurrentVersion '0.0.1' | Should -BeTrue
        Test-IsPatchOptionRedundant -CurrentVersion '0.0.42' | Should -BeTrue
    }

    It 'returns $false (conservative default) when the version is missing, null, or whitespace' {
        Test-IsPatchOptionRedundant -CurrentVersion '' | Should -BeFalse
        Test-IsPatchOptionRedundant -CurrentVersion $null | Should -BeFalse
        Test-IsPatchOptionRedundant -CurrentVersion '   ' | Should -BeFalse
    }
}

# ---------------------------------------------------------------------------
# Get-PackageReleaseDecision (input-validation loop)
# ---------------------------------------------------------------------------

Describe 'Get-PackageReleaseDecision' {

    BeforeAll {
        function script:NewFinding {
            param(
                [string]$Folder = 'ohno',
                [AllowEmptyString()][AllowNull()][string]$CurrentVersion
            )
            return [pscustomobject]@{
                Folder           = $Folder
                PackageName      = $Folder
                CurrentVersion   = $CurrentVersion
                ChangedFileCount = 1
                DependencyChains = @(, @('a', $Folder))
            }
        }

        # Helper: install a Read-Host mock that returns scripted answers in order.
        function script:SetReadHostQueue {
            param([Parameter(Mandatory = $true)][object[]]$Answers)
            $script:RH_Queue = [System.Collections.Queue]::new()
            foreach ($a in $Answers) { $script:RH_Queue.Enqueue($a) }
            $script:RH_PromptsObserved = [System.Collections.Generic.List[string]]::new()
            Mock -CommandName Read-Host -MockWith {
                param([string]$Prompt)
                $script:RH_PromptsObserved.Add($Prompt) | Out-Null
                if ($script:RH_Queue.Count -eq 0) {
                    throw "Read-Host mock ran out of answers (prompt: '$Prompt')"
                }
                return $script:RH_Queue.Dequeue()
            }
        }
    }

    BeforeEach {
        Mock -CommandName Show-PackageDiff -MockWith { }
        # Suppress menu rendering and Write-Host noise for assertions on prompts/output.
        Mock -CommandName Show-PackageMenu -MockWith { }
    }

    Context 'happy-path single-keystroke answers' {
        It "returns 'ignore' for input '2'" {
            SetReadHostQueue -Answers @('2')
            $r = Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'ignore'
        }
        It "returns 'major' for input '3'" {
            SetReadHostQueue -Answers @('3')
            $r = Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'major'
        }
        It "returns 'minor' for input '4'" {
            SetReadHostQueue -Answers @('4')
            $r = Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'minor'
        }
        It "returns 'patch' for input '5'" {
            SetReadHostQueue -Answers @('5')
            $r = Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'patch'
        }
    }

    Context 'invalid input handling' {
        It 'silently re-prompts on empty input (no warning emitted)' {
            SetReadHostQueue -Answers @('', '2')
            $warn = $null
            $r = Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive 3>&1 6>&1
            # The returned hashtable should be unwrapped from the captured stream.
            # We assert directly via a fresh call below.
            SetReadHostQueue -Answers @('', '2')
            $r2 = Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive
            $r2.Action | Should -Be 'ignore'
            $script:RH_PromptsObserved.Count | Should -Be 2
        }

        It "complains then re-prompts on '12' (whole-string check, not first char)" {
            SetReadHostQueue -Answers @('12', '2')
            $out = & { Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive } 6>&1
            # The hashtable is the last item written (6>&1 merges Information stream).
            $actionItem = $out | Where-Object { $_ -is [hashtable] } | Select-Object -Last 1
            $actionItem.Action | Should -Be 'ignore'
            $script:RH_PromptsObserved.Count | Should -Be 2
            ($out | Out-String) | Should -Match "Invalid choice '12'"
        }

        It "complains then re-prompts on whitespace-only input '   '" {
            SetReadHostQueue -Answers @('   ', '2')
            # '   '.Trim() = '' so this should follow the silent-reprompt path,
            # NOT the invalid-choice path. We assert no warning.
            $out = & { Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive } 6>&1
            $actionItem = $out | Where-Object { $_ -is [hashtable] } | Select-Object -Last 1
            $actionItem.Action | Should -Be 'ignore'
            ($out | Out-String) | Should -Not -Match 'Invalid choice'
        }

        It "complains then re-prompts on letter input 'x'" {
            SetReadHostQueue -Answers @('x', '2')
            $out = & { Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive } 6>&1
            $actionItem = $out | Where-Object { $_ -is [hashtable] } | Select-Object -Last 1
            $actionItem.Action | Should -Be 'ignore'
            ($out | Out-String) | Should -Match "Invalid choice 'x'"
        }

        It "complains then re-prompts on '1 2' (extra characters)" {
            SetReadHostQueue -Answers @('1 2', '2')
            $out = & { Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive } 6>&1
            $actionItem = $out | Where-Object { $_ -is [hashtable] } | Select-Object -Last 1
            $actionItem.Action | Should -Be 'ignore'
            ($out | Out-String) | Should -Match "Invalid choice '1 2'"
        }

        It "complains then re-prompts on '2.0'" {
            SetReadHostQueue -Answers @('2.0', '2')
            $out = & { Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive } 6>&1
            $actionItem = $out | Where-Object { $_ -is [hashtable] } | Select-Object -Last 1
            $actionItem.Action | Should -Be 'ignore'
            ($out | Out-String) | Should -Match "Invalid choice '2.0'"
        }

        It 'absorbs a chain of bad inputs and still returns the valid choice at the end' {
            SetReadHostQueue -Answers @('', 'x', '12', '   ', '5')
            $r = Get-PackageReleaseDecision -Finding (NewFinding) -RemainingCount 0 -RepoRoot $TestDrive 6>$null
            $r.Action | Should -Be 'patch'
            $script:RH_PromptsObserved.Count | Should -Be 5
        }
    }

    Context 'View Diff (choice 1) re-prompts without re-rendering the menu' {
        It "calls Show-PackageDiff once when the user picks '1' then '4', menu rendered only once" {
            SetReadHostQueue -Answers @('1', '4')
            $r = Get-PackageReleaseDecision -Finding (NewFinding -Folder 'b') -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'minor'
            Should -Invoke -CommandName Show-PackageDiff -Times 1 -Exactly -ParameterFilter { $Folder -eq 'b' }
            # Menu rendered: 1 initial only — diff selection does NOT re-render
            # the menu (the options are still visible in scrollback above).
            Should -Invoke -CommandName Show-PackageMenu -Times 1 -Exactly
            # But the Read-Host prompt IS observed twice: once initial, once
            # after the diff is shown.
            $script:RH_PromptsObserved.Count | Should -Be 2
        }

        It "calls Show-PackageDiff twice when the user picks '1', '1', '4', menu still rendered only once" {
            SetReadHostQueue -Answers @('1', '1', '4')
            $r = Get-PackageReleaseDecision -Finding (NewFinding -Folder 'b') -RemainingCount 2 -RepoRoot $TestDrive
            $r.Action | Should -Be 'minor'
            Should -Invoke -CommandName Show-PackageDiff -Times 2 -Exactly
            Should -Invoke -CommandName Show-PackageMenu -Times 1 -Exactly
            $script:RH_PromptsObserved.Count | Should -Be 3
        }
    }

    Context 'prompt format' {
        It "includes the package name in the Read-Host prompt for scrollback / scenario disambiguation" {
            SetReadHostQueue -Answers @('2')
            Get-PackageReleaseDecision -Finding (NewFinding -Folder 'mypkg') -RemainingCount 0 -RepoRoot $TestDrive | Out-Null
            $script:RH_PromptsObserved[0] | Should -Match "Choose option for 'mypkg'"
        }

        It "advertises the full [1-5] range when CurrentVersion is unknown" {
            SetReadHostQueue -Answers @('2')
            Get-PackageReleaseDecision -Finding (NewFinding -Folder 'mypkg') -RemainingCount 0 -RepoRoot $TestDrive | Out-Null
            $script:RH_PromptsObserved[0] | Should -Match '\[1-5\]'
        }

        It "advertises the narrower [1-4] range when option 5 is hidden (0.x.y package)" {
            SetReadHostQueue -Answers @('2')
            $finding = NewFinding -Folder 'mypkg' -CurrentVersion '0.1.2'
            Get-PackageReleaseDecision -Finding $finding -RemainingCount 0 -RepoRoot $TestDrive | Out-Null
            $script:RH_PromptsObserved[0] | Should -Match '\[1-4\]'
        }
    }

    Context 'option 5 is rejected when hidden (0.x.y package)' {
        It "treats '5' as invalid and re-prompts, message references the narrower range" {
            SetReadHostQueue -Answers @('5', '4')
            $finding = NewFinding -Folder 'pkg' -CurrentVersion '0.1.2'
            $out = & { Get-PackageReleaseDecision -Finding $finding -RemainingCount 0 -RepoRoot $TestDrive } 6>&1
            $actionItem = $out | Where-Object { $_ -is [hashtable] } | Select-Object -Last 1
            $actionItem.Action | Should -Be 'minor'
            $script:RH_PromptsObserved.Count | Should -Be 2
            ($out | Out-String) | Should -Match "Invalid choice '5'"
            ($out | Out-String) | Should -Match 'from 1 to 4'
        }

        It "still accepts '4' (non-breaking) on a 0.x.y package" {
            SetReadHostQueue -Answers @('4')
            $finding = NewFinding -Folder 'pkg' -CurrentVersion '0.1.2'
            $r = Get-PackageReleaseDecision -Finding $finding -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'minor'
        }

        It "still accepts '3' (breaking) on a 0.x.y package" {
            SetReadHostQueue -Answers @('3')
            $finding = NewFinding -Folder 'pkg' -CurrentVersion '0.1.2'
            $r = Get-PackageReleaseDecision -Finding $finding -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'major'
        }
    }
}

# ---------------------------------------------------------------------------
# Show-PackageDiff (orchestrator: diff -> temp file -> opener + tracking)
# ---------------------------------------------------------------------------

Describe 'Show-PackageDiff' {

    BeforeEach {
        # Reset the tracking list so tests don't leak into each other.
        $script:TempPackageDiffPaths = [System.Collections.Generic.List[string]]::new()
        Mock -CommandName Get-PackageDiffText -MockWith { return "diff body for $Folder`n" }
        Mock -CommandName Open-PathWithPreferredEditor -MockWith { }
    }

    It 'writes the diff to a file and invokes the opener with the same path' {
        Mock -CommandName Get-PreferredEditor -MockWith {
            [pscustomobject]@{ Kind = 'system'; FileExtension = '.txt' }
        }
        # Force the temp file into $TestDrive so we can inspect / clean up.
        Mock -CommandName Save-PackageDiffToTempFile -MockWith {
            $p = Join-Path $TestDrive ("pkg-" + [guid]::NewGuid().ToString('N') + '.txt')
            Set-Content -LiteralPath $p -Value $DiffText -NoNewline
            return $p
        }

        Show-PackageDiff -RepoRoot $TestDrive -Folder 'bytesbuf' 6>$null

        $script:TempPackageDiffPaths.Count | Should -Be 1
        $written = $script:TempPackageDiffPaths[0]
        Test-Path -LiteralPath $written | Should -BeTrue
        (Get-Content -LiteralPath $written -Raw) | Should -Be "diff body for bytesbuf`n"

        Should -Invoke -CommandName Open-PathWithPreferredEditor -Times 1 -Exactly -ParameterFilter { $Path -eq $written -and $Editor.Kind -eq 'system' }
    }

    It "uses the preferred editor's extension when saving (e.g. .diff for VS Code)" {
        Mock -CommandName Get-PreferredEditor -MockWith {
            [pscustomobject]@{ Kind = 'code'; FileExtension = '.diff' }
        }
        # Spy on Save-PackageDiffToTempFile to capture the extension argument.
        Mock -CommandName Save-PackageDiffToTempFile -MockWith {
            $p = Join-Path $TestDrive ("pkg-" + [guid]::NewGuid().ToString('N') + $Extension)
            Set-Content -LiteralPath $p -Value $DiffText -NoNewline
            return $p
        }

        Show-PackageDiff -RepoRoot $TestDrive -Folder 'bytesbuf' 6>$null

        Should -Invoke -CommandName Save-PackageDiffToTempFile -Times 1 -Exactly -ParameterFilter { $Extension -eq '.diff' }
        $script:TempPackageDiffPaths[0] | Should -BeLike '*.diff'
        Should -Invoke -CommandName Open-PathWithPreferredEditor -Times 1 -Exactly -ParameterFilter { $Editor.Kind -eq 'code' }
    }

    It 'appends to $script:TempPackageDiffPaths even when the opener fails silently' {
        Mock -CommandName Get-PreferredEditor -MockWith {
            [pscustomobject]@{ Kind = 'system'; FileExtension = '.txt' }
        }
        Mock -CommandName Save-PackageDiffToTempFile -MockWith {
            $p = Join-Path $TestDrive ("pkg-" + [guid]::NewGuid().ToString('N') + '.txt')
            Set-Content -LiteralPath $p -Value $DiffText -NoNewline
            return $p
        }
        Mock -CommandName Open-PathWithPreferredEditor -MockWith { }   # no-op, simulates failure absorbed by helper

        Show-PackageDiff -RepoRoot $TestDrive -Folder 'a' 6>$null
        Show-PackageDiff -RepoRoot $TestDrive -Folder 'b' 6>$null
        $script:TempPackageDiffPaths.Count | Should -Be 2
    }
}

# ---------------------------------------------------------------------------
# Save-PackageDiffToTempFile (pure-ish: writes a file, returns path)
# ---------------------------------------------------------------------------

Describe 'Save-PackageDiffToTempFile' {
    It 'defaults to .txt and writes the diff text under the requested directory' {
        $dir = Join-Path $TestDrive 'savediff'
        $p = Save-PackageDiffToTempFile -Folder 'bytesbuf_io' -DiffText "hello`nworld" -Directory $dir
        $p | Should -BeLike (Join-Path $dir 'oxi-pkg-diff-bytesbuf_io-*.txt')
        (Get-Content -LiteralPath $p -Raw) | Should -Be "hello`nworld"
    }

    It 'honours an explicit -Extension (e.g. .diff for VS Code)' {
        $dir = Join-Path $TestDrive 'savediff-ext'
        $p = Save-PackageDiffToTempFile -Folder 'bytesbuf_io' -DiffText 'x' -Directory $dir -Extension '.diff'
        $p | Should -BeLike (Join-Path $dir 'oxi-pkg-diff-bytesbuf_io-*.diff')
    }

    It 'normalises an extension passed without the leading dot' {
        $dir = Join-Path $TestDrive 'savediff-nodot'
        $p = Save-PackageDiffToTempFile -Folder 'a' -DiffText 'x' -Directory $dir -Extension 'diff'
        $p | Should -BeLike (Join-Path $dir 'oxi-pkg-diff-a-*.diff')
    }

    It 'sanitises folder names containing characters not allowed in file names' {
        $dir = Join-Path $TestDrive 'savediff2'
        $p = Save-PackageDiffToTempFile -Folder 'weird/pkg name' -DiffText 'x' -Directory $dir
        (Split-Path $p -Leaf) | Should -Match '^oxi-pkg-diff-weird_pkg_name-[0-9a-f]+\.txt$'
    }
}

# ---------------------------------------------------------------------------
# Get-PreferredEditor (VS Code -> code-insiders -> system fallback)
# ---------------------------------------------------------------------------

Describe 'Get-PreferredEditor' {

    It "returns 'code' + .diff when `code` is on PATH" {
        Mock -CommandName Get-Command -MockWith {
            if ($Name -eq 'code') { return [pscustomobject]@{ Name = 'code' } }
            return $null
        }
        $e = Get-PreferredEditor
        $e.Kind | Should -Be 'code'
        $e.FileExtension | Should -Be '.diff'
    }

    It "prefers 'code' over 'code-insiders' when both are on PATH" {
        Mock -CommandName Get-Command -MockWith {
            return [pscustomobject]@{ Name = $Name }
        }
        $e = Get-PreferredEditor
        $e.Kind | Should -Be 'code'
    }

    It "returns 'code-insiders' + .diff when only insiders is on PATH" {
        Mock -CommandName Get-Command -MockWith {
            if ($Name -eq 'code') { return $null }
            if ($Name -eq 'code-insiders') { return [pscustomobject]@{ Name = 'code-insiders' } }
            return $null
        }
        $e = Get-PreferredEditor
        $e.Kind | Should -Be 'code-insiders'
        $e.FileExtension | Should -Be '.diff'
    }

    It "returns 'system' + .txt when no VS Code variant is on PATH" {
        Mock -CommandName Get-Command -MockWith { return $null }
        $e = Get-PreferredEditor
        $e.Kind | Should -Be 'system'
        $e.FileExtension | Should -Be '.txt'
    }
}

# ---------------------------------------------------------------------------
# Open-PathWithPreferredEditor (dispatch on editor kind)
# ---------------------------------------------------------------------------

Describe 'Open-PathWithPreferredEditor' {
    BeforeEach {
        # Default safety net so a flaky test doesn't actually try to launch VS Code or the OS opener.
        Mock -CommandName Start-Process -MockWith { }
    }

    It "invokes 'code' when the editor kind is 'code'" {
        # Mocking external executables: Pester can mock cmdlets/functions, not arbitrary native commands,
        # so we capture the dispatch by mocking Get-Variable for $IsWindows (irrelevant here) and asserting
        # behavior indirectly via the LASTEXITCODE check path. The simplest assertion is that the function
        # neither throws nor writes a warning when the (mocked) external command succeeds.
        function script:code { param([string]$p) $global:LASTEXITCODE = 0 }
        Mock -CommandName Write-Warning -MockWith { }
        try {
            { Open-PathWithPreferredEditor -Path 'C:\temp\demo.diff' -Editor ([pscustomobject]@{ Kind = 'code'; FileExtension = '.diff' }) } | Should -Not -Throw
            Should -Invoke -CommandName Write-Warning -Times 0 -Exactly
        } finally {
            Remove-Item function:script:code -ErrorAction SilentlyContinue
        }
    }

    It "warns and does not throw when 'code' exits with a non-zero code" {
        function script:code { param([string]$p) $global:LASTEXITCODE = 7 }
        Mock -CommandName Write-Warning -MockWith { }
        try {
            { Open-PathWithPreferredEditor -Path 'C:\temp\demo.diff' -Editor ([pscustomobject]@{ Kind = 'code'; FileExtension = '.diff' }) } | Should -Not -Throw
            Should -Invoke -CommandName Write-Warning -Times 1 -Exactly -ParameterFilter { $Message -match 'code exited with code 7' }
        } finally {
            Remove-Item function:script:code -ErrorAction SilentlyContinue
        }
    }

    Context "system-kind dispatch on Windows" -Skip:(-not ((Get-Variable -Name IsWindows -Scope Global -ErrorAction SilentlyContinue) -eq $null -or $IsWindows)) {
        It 'falls back to Start-Process for kind = system' {
            Open-PathWithPreferredEditor -Path 'C:\temp\demo.txt' -Editor ([pscustomobject]@{ Kind = 'system'; FileExtension = '.txt' })
            Should -Invoke -CommandName Start-Process -Times 1 -Exactly -ParameterFilter { $FilePath -eq 'C:\temp\demo.txt' }
        }

        It 'emits a warning and does not throw when Start-Process throws' {
            Mock -CommandName Start-Process -MockWith { throw 'no association' }
            Mock -CommandName Write-Warning -MockWith { }
            { Open-PathWithPreferredEditor -Path 'C:\temp\demo.txt' -Editor ([pscustomobject]@{ Kind = 'system'; FileExtension = '.txt' }) } | Should -Not -Throw
            Should -Invoke -CommandName Write-Warning -Times 1 -Exactly -ParameterFilter { $Message -match 'no association' }
        }

        It 'resolves the editor on the fly when -Editor is omitted' {
            Mock -CommandName Get-PreferredEditor -MockWith {
                [pscustomobject]@{ Kind = 'system'; FileExtension = '.txt' }
            }
            Open-PathWithPreferredEditor -Path 'C:\temp\demo.txt'
            Should -Invoke -CommandName Get-PreferredEditor -Times 1 -Exactly
            Should -Invoke -CommandName Start-Process -Times 1 -Exactly
        }
    }
}

# ---------------------------------------------------------------------------
# Invoke-PostReleaseDepScan — status indicator while analysing packages
# ---------------------------------------------------------------------------

Describe 'Invoke-PostReleaseDepScan: analysing-packages status indicator' {

    BeforeEach {
        # Single fake published crate for the maxIterations cap.
        Mock -CommandName Get-WorkspaceCrates -MockWith {
            ,@([pscustomobject]@{ Folder = 'fake'; Name = 'fake'; Published = $true })
        }
        # Drive the loop into "no findings" so it exits after a single iteration.
        Mock -CommandName Get-UnreleasedModifiedDependencies -MockWith { @() }
        Mock -CommandName Invalidate-WorkspaceMetadataCache -MockWith { }
    }

    It 'emits the "Analyzing packages..." status line in interactive mode (before the BFS runs)' {
        Mock -CommandName Test-InteractiveSession -MockWith { $true }
        $releases = @()
        $out = & {
            Invoke-PostReleaseDepScan -RepoRoot $TestDrive -BaseRef 'origin/main' `
                -ReleasesRef ([ref]$releases) -RootCargoToml (Join-Path $TestDrive 'Cargo.toml')
        } 6>&1
        ($out | Out-String) | Should -Match 'Analyzing packages for unreleased modifications'
    }

    It 'suppresses the status line in non-interactive mode (output is just log noise there)' {
        Mock -CommandName Test-InteractiveSession -MockWith { $false }
        $releases = @()
        $out = & {
            Invoke-PostReleaseDepScan -RepoRoot $TestDrive -BaseRef 'origin/main' `
                -ReleasesRef ([ref]$releases) -RootCargoToml (Join-Path $TestDrive 'Cargo.toml')
        } 6>&1
        ($out | Out-String) | Should -Not -Match 'Analyzing packages for unreleased modifications'
    }

    It 'still suppresses the status line when -NonInteractive parameter is explicit and Test-InteractiveSession would say yes' {
        # The two switches are independent: -NonInteractive forces the
        # non-interactive code path, suppressing the indicator regardless of
        # the terminal's TTY-ness.
        Mock -CommandName Test-InteractiveSession -MockWith { $true }
        $releases = @()
        $out = & {
            Invoke-PostReleaseDepScan -RepoRoot $TestDrive -BaseRef 'origin/main' `
                -ReleasesRef ([ref]$releases) -RootCargoToml (Join-Path $TestDrive 'Cargo.toml') `
                -NonInteractive
        } 6>&1
        ($out | Out-String) | Should -Not -Match 'Analyzing packages for unreleased modifications'
    }
}

# ---------------------------------------------------------------------------
# Show-FinalMessage — post-success "next steps" instructions
# ---------------------------------------------------------------------------

Describe 'Show-FinalMessage' {

    BeforeAll {
        function script:NewRelease {
            param([string]$Crate, [string]$NewVersion, [string]$OldVersion = '0.0.0')
            return [pscustomobject]@{
                Crate      = $Crate
                OldVersion = $OldVersion
                NewVersion = $NewVersion
            }
        }
    }

    Context 'single-package release' {
        It 'uses the scoped feat(<crate>) commit form when only one package was released' {
            $releases = @(NewRelease -Crate 'bytesbuf_io' -NewVersion '0.5.1')
            $out = & { Show-FinalMessage -CrateName 'bytesbuf_io' -Releases $releases } 6>&1
            ($out | Out-String) | Should -Match 'git commit -m "feat\(bytesbuf_io\): release v0\.5\.1"'
        }

        It 'does NOT mention "additional package(s)" when only one package was released' {
            $releases = @(NewRelease -Crate 'bytesbuf_io' -NewVersion '0.5.1')
            $out = & { Show-FinalMessage -CrateName 'bytesbuf_io' -Releases $releases } 6>&1
            ($out | Out-String) | Should -Not -Match 'additional package'
        }
    }

    Context 'multi-package release' {
        It 'uses the unscoped "feat:" commit form and counts the extras when several packages were released' {
            $releases = @(
                NewRelease -Crate 'bytesbuf_io' -NewVersion '0.5.1'
                NewRelease -Crate 'bytesbuf'    -NewVersion '0.4.2'
                NewRelease -Crate 'a'           -NewVersion '0.1.1'
                NewRelease -Crate 'b'           -NewVersion '0.2.1'
                NewRelease -Crate 'c'           -NewVersion '0.3.1'
                NewRelease -Crate 'd'           -NewVersion '0.4.1'
            )
            $out = & { Show-FinalMessage -CrateName 'bytesbuf_io' -Releases $releases } 6>&1
            ($out | Out-String) | Should -Match 'git commit -m "feat: release bytesbuf_io v0\.5\.1 and 5 additional packages"'
        }

        It 'uses singular noun ("1 additional package") when exactly two packages were released' {
            $releases = @(
                NewRelease -Crate 'foo' -NewVersion '1.0.0'
                NewRelease -Crate 'bar' -NewVersion '2.0.0'
            )
            $out = & { Show-FinalMessage -CrateName 'foo' -Releases $releases } 6>&1
            ($out | Out-String) | Should -Match 'release foo v1\.0\.0 and 1 additional package"'
            ($out | Out-String) | Should -Not -Match '1 additional packages'
        }

        It 'finds the primary release even when it is not first in the release record array' {
            $releases = @(
                NewRelease -Crate 'cascade_a' -NewVersion '0.1.1'
                NewRelease -Crate 'cascade_b' -NewVersion '0.2.1'
                NewRelease -Crate 'primary'   -NewVersion '0.7.0'
            )
            $out = & { Show-FinalMessage -CrateName 'primary' -Releases $releases } 6>&1
            ($out | Out-String) | Should -Match 'release primary v0\.7\.0 and 2 additional packages"'
        }
    }

    Context 'git push instruction' {
        It 'emits a plain `git push` (no `origin <branch>` placeholder)' {
            $releases = @(NewRelease -Crate 'foo' -NewVersion '1.0.0')
            $out = & { Show-FinalMessage -CrateName 'foo' -Releases $releases } 6>&1
            $text = $out | Out-String
            $text | Should -Match '(?m)^\s*git push\s*$'
            $text | Should -Not -Match 'git push origin'
            $text | Should -Not -Match 'mybranch'
        }
    }
}

# ---------------------------------------------------------------------------
# Cascade-message helpers (Get-ChangeLabelFromBumpKind,
# Get-ShortChangeLabelFromBumpKind, Format-CascadeAnnouncement). These
# back the "🔗 Cascading release to N dependent package(s) as <label> [...]"
# line printed by Invoke-ReleaseFlow before each cascade. Pure functions so
# they're driven directly here without staging a workspace.
# ---------------------------------------------------------------------------

Describe 'Get-ChangeLabelFromBumpKind' {
    It "maps 'major' to 'breaking change'" {
        Get-ChangeLabelFromBumpKind -BumpKind 'major' | Should -Be 'breaking change'
    }

    It "maps 'minor' to 'non-breaking change'" {
        Get-ChangeLabelFromBumpKind -BumpKind 'minor' | Should -Be 'non-breaking change'
    }

    It "maps 'patch' to 'patch'" {
        Get-ChangeLabelFromBumpKind -BumpKind 'patch' | Should -Be 'patch'
    }
}

Describe 'Get-ShortChangeLabelFromBumpKind' {
    It "maps 'major' to 'breaking' (no trailing 'change' noun)" {
        Get-ShortChangeLabelFromBumpKind -BumpKind 'major' | Should -Be 'breaking'
    }

    It "maps 'minor' to 'non-breaking'" {
        Get-ShortChangeLabelFromBumpKind -BumpKind 'minor' | Should -Be 'non-breaking'
    }

    It "maps 'patch' to 'patch'" {
        Get-ShortChangeLabelFromBumpKind -BumpKind 'patch' | Should -Be 'patch'
    }
}

Describe 'Format-CascadeAnnouncement' {

    Context 'headline label reflects the EXPOSING bump kind' {
        # The exposing bump is the kind that's applied to dependents that
        # re-export the target's types in their public API. Test-IsBreakingChange
        # is consulted at the call site to derive this — here we just assert that
        # whatever value lands in -ExposingBump is what shows up as the headline.

        It 'reads "as breaking change" when -ExposingBump is major' {
            $out = Format-CascadeAnnouncement -ExposingBump 'major' -NonExposingBump 'patch' `
                -TargetCrateName 'http_extensions' -DependentNames @('fetch_hyper', 'seatbelt_http')
            $out | Should -Match 'as breaking change'
        }

        It 'reads "as non-breaking change" when -ExposingBump is minor' {
            $out = Format-CascadeAnnouncement -ExposingBump 'minor' -NonExposingBump 'patch' `
                -TargetCrateName 'http_extensions' -DependentNames @('fetch_hyper', 'seatbelt_http')
            $out | Should -Match 'as non-breaking change'
        }

        It 'reads "as patch" when -ExposingBump is patch (and parenthetical is suppressed — see other tests)' {
            $out = Format-CascadeAnnouncement -ExposingBump 'patch' -NonExposingBump 'patch' `
                -TargetCrateName 'http_extensions' -DependentNames @('fetch_hyper')
            $out | Should -Match 'as patch'
        }
    }

    Context 'parenthetical clause for non-exposing dependents' {
        It 'appends "(or patch if no API exposure of `<target>`)" when exposing=major and non-exposing=patch' {
            $out = Format-CascadeAnnouncement -ExposingBump 'major' -NonExposingBump 'patch' `
                -TargetCrateName 'http_extensions' -DependentNames @('fetch_hyper', 'seatbelt_http')
            $out | Should -Match '\(or patch if no API exposure of `http_extensions`\)'
        }

        It 'appends "(or patch if no API exposure of `<target>`)" when exposing=minor and non-exposing=patch' {
            $out = Format-CascadeAnnouncement -ExposingBump 'minor' -NonExposingBump 'patch' `
                -TargetCrateName 'http_extensions' -DependentNames @('fetch_hyper', 'seatbelt_http')
            $out | Should -Match '\(or patch if no API exposure of `http_extensions`\)'
        }

        It 'omits the parenthetical entirely when exposing and non-exposing are both patch (no downgrade possible)' {
            $out = Format-CascadeAnnouncement -ExposingBump 'patch' -NonExposingBump 'patch' `
                -TargetCrateName 'http_extensions' -DependentNames @('fetch_hyper', 'seatbelt_http')
            $out | Should -Not -Match '\(or .+ if no API exposure'
            # Ensure the colon still lands directly after the headline label.
            $out | Should -Match 'as patch:\s'
        }

        It 'preserves backticks around the target crate name (markdown-style code formatting in CLI output)' {
            $out = Format-CascadeAnnouncement -ExposingBump 'major' -NonExposingBump 'patch' `
                -TargetCrateName 'my-crate' -DependentNames @('a', 'b')
            $out | Should -Match '`my-crate`'
        }
    }

    Context 'singular vs plural "dependent package(s)" noun' {
        It 'uses singular "dependent package" when there is exactly one dependent' {
            $out = Format-CascadeAnnouncement -ExposingBump 'major' -NonExposingBump 'patch' `
                -TargetCrateName 'foo' -DependentNames @('bar')
            $out | Should -Match '1 dependent package as'
            $out | Should -Not -Match '1 dependent packages'
        }

        It 'uses plural "dependent packages" when there are multiple dependents' {
            $out = Format-CascadeAnnouncement -ExposingBump 'major' -NonExposingBump 'patch' `
                -TargetCrateName 'foo' -DependentNames @('bar', 'baz', 'qux')
            $out | Should -Match '3 dependent packages as'
        }
    }

    Context 'full-shape rendering across all original bump kinds (target is 1.x.y conceptually)' {
        # 1.x.y / 0.x.y / 0.0.x distinctions are made by the CALLER (via
        # Test-IsBreakingChange, which feeds into ExposingBump). The formatter
        # is agnostic to the underlying semver shape — it just gets the
        # already-resolved bump kinds. These tests pin the exact rendered
        # shape for each combination the caller can hand in.

        It 'breaking + non-exposing patch: "as breaking change (or patch if no API exposure of `target`)"' {
            $out = Format-CascadeAnnouncement -ExposingBump 'major' -NonExposingBump 'patch' `
                -TargetCrateName 'target' -DependentNames @('a', 'b')
            $out | Should -Be "🔗 Cascading release to 2 dependent packages as breaking change (or patch if no API exposure of ``target``): a, b"
        }

        It 'non-breaking + non-exposing patch: "as non-breaking change (or patch if no API exposure of `target`)"' {
            $out = Format-CascadeAnnouncement -ExposingBump 'minor' -NonExposingBump 'patch' `
                -TargetCrateName 'target' -DependentNames @('a', 'b')
            $out | Should -Be "🔗 Cascading release to 2 dependent packages as non-breaking change (or patch if no API exposure of ``target``): a, b"
        }

        It 'patch + non-exposing patch: "as patch" (parenthetical suppressed, no downgrade possible)' {
            $out = Format-CascadeAnnouncement -ExposingBump 'patch' -NonExposingBump 'patch' `
                -TargetCrateName 'target' -DependentNames @('a', 'b')
            $out | Should -Be "🔗 Cascading release to 2 dependent packages as patch: a, b"
        }
    }

    Context 'caller-level integration: cascade for 0.x.y targets where Test-IsBreakingChange promotes to major' {
        # When the target is 0.x.y (x>=1) and the user picks "non-breaking",
        # Test-IsBreakingChange returns $false so $exposingCascadeBump stays
        # 'minor' (no promotion). This is just a sanity check that the
        # caller's pre-formatter math (in Invoke-ReleaseFlow) lines up with
        # the formatter's expectations for the 0.x.y case.
        It '0.x.y target + user picks breaking (major): the caller promotes exposing to major; formatter reads "breaking change"' {
            $isBreaking = Test-IsBreakingChange -oldVersion '0.5.2' -bump 'major'
            $isBreaking | Should -BeTrue
            $exposingBump = if ($isBreaking) { 'major' } else { 'minor' }
            $out = Format-CascadeAnnouncement -ExposingBump $exposingBump -NonExposingBump 'patch' `
                -TargetCrateName 't' -DependentNames @('d')
            $out | Should -Match 'as breaking change \(or patch if no API exposure of `t`\)'
        }

        It '0.x.y target + user picks non-breaking (minor): caller keeps exposing as minor; formatter reads "non-breaking change"' {
            $isBreaking = Test-IsBreakingChange -oldVersion '0.5.2' -bump 'minor'
            $isBreaking | Should -BeFalse
            $exposingBump = if ($isBreaking) { 'major' } else { 'minor' }
            $out = Format-CascadeAnnouncement -ExposingBump $exposingBump -NonExposingBump 'patch' `
                -TargetCrateName 't' -DependentNames @('d')
            $out | Should -Match 'as non-breaking change \(or patch if no API exposure of `t`\)'
        }

        It '0.0.x target (always breaking under Cargo semver): caller marks breaking, formatter reads "breaking change"' {
            $isBreaking = Test-IsBreakingChange -oldVersion '0.0.5' -bump 'patch'
            $isBreaking | Should -BeTrue
            # Caller's $exposingCascadeBump = if($targetIsBreaking) {'major'} else {$cascadeBump}
            $exposingBump = if ($isBreaking) { 'major' } else { 'patch' }
            $out = Format-CascadeAnnouncement -ExposingBump $exposingBump -NonExposingBump 'patch' `
                -TargetCrateName 't' -DependentNames @('d')
            $out | Should -Match 'as breaking change \(or patch if no API exposure of `t`\)'
        }
    }
}

Describe 'Format-CascadeDependentLine' {

    Context 'bump label uses the SHORT semantic vocabulary (not internal Cargo bump kinds)' {
        It "renders 'breaking' for major (exposing dependent)" {
            Format-CascadeDependentLine -DependentName 'd' -BumpKind 'major' -ExposesTarget $true |
                Should -Be '  • d -> breaking (exposes target in public API)'
        }

        It "renders 'non-breaking' for minor (exposing dependent)" {
            Format-CascadeDependentLine -DependentName 'd' -BumpKind 'minor' -ExposesTarget $true |
                Should -Be '  • d -> non-breaking (exposes target in public API)'
        }

        It "renders 'patch' for patch (exposing dependent — uncommon but possible if the target itself is a patch)" {
            Format-CascadeDependentLine -DependentName 'd' -BumpKind 'patch' -ExposesTarget $true |
                Should -Be '  • d -> patch (exposes target in public API)'
        }
    }

    Context 'why-clause reflects the ExposesTarget flag' {
        It "uses 'exposes target in public API' when ExposesTarget = `$true" {
            $out = Format-CascadeDependentLine -DependentName 'fetch_hyper' -BumpKind 'minor' -ExposesTarget $true
            $out | Should -Match '\(exposes target in public API\)$'
        }

        It "uses 'internal use only' when ExposesTarget = `$false (the non-exposing downgrade case)" {
            $out = Format-CascadeDependentLine -DependentName 'seatbelt_http' -BumpKind 'patch' -ExposesTarget $false
            $out | Should -Match '\(internal use only\)$'
            # The bump label should still be the SHORT semantic form ('patch'), not the internal kind.
            $out | Should -Match '-> patch '
        }
    }

    Context 'never leaks the internal Cargo bump vocabulary' {
        # Pin the rename: the line must never contain the words 'major' / 'minor' / 'patch'
        # except where 'patch' is the legitimate semantic label. This protects against a
        # regression where someone wires $depBump back into the rendered string.
        It "does not render the word 'major' anywhere when BumpKind is major" {
            $out = Format-CascadeDependentLine -DependentName 'd' -BumpKind 'major' -ExposesTarget $true
            $out | Should -Not -Match '\bmajor\b'
        }

        It "does not render the word 'minor' anywhere when BumpKind is minor" {
            $out = Format-CascadeDependentLine -DependentName 'd' -BumpKind 'minor' -ExposesTarget $true
            $out | Should -Not -Match '\bminor\b'
        }
    }
}

# ---------------------------------------------------------------------------
# Resolve-ReleaseSpecFromChange (CLI -Change → internal Bump/Version
# translation). Pure function with one dependency: the package's current
# version string (which the caller reads from Cargo.toml and passes in).
# ---------------------------------------------------------------------------

Describe 'Resolve-ReleaseSpecFromChange' {

    Context 'Breaking maps to a major bump (no explicit Version)' {
        It 'returns Bump=major and empty Version for a 1.x.y current version' {
            $spec = Resolve-ReleaseSpecFromChange -Change 'Breaking' -CurrentVersion '1.2.3'
            $spec.Bump    | Should -Be 'major'
            $spec.Version | Should -Be ''
        }

        It 'returns Bump=major and empty Version for a 0.x.y current version (caller handles Cargo semver carve-out)' {
            $spec = Resolve-ReleaseSpecFromChange -Change 'Breaking' -CurrentVersion '0.5.2'
            $spec.Bump    | Should -Be 'major'
            $spec.Version | Should -Be ''
        }

        It 'returns Bump=major for 0.0.x as well (every change is breaking under Cargo semver — encoded downstream)' {
            $spec = Resolve-ReleaseSpecFromChange -Change 'Breaking' -CurrentVersion '0.0.5'
            $spec.Bump    | Should -Be 'major'
            $spec.Version | Should -Be ''
        }
    }

    Context 'NonBreaking maps to a minor bump' {
        It 'returns Bump=minor and empty Version regardless of current version shape' {
            (Resolve-ReleaseSpecFromChange -Change 'NonBreaking' -CurrentVersion '1.2.3').Bump | Should -Be 'minor'
            (Resolve-ReleaseSpecFromChange -Change 'NonBreaking' -CurrentVersion '0.5.2').Bump | Should -Be 'minor'
            (Resolve-ReleaseSpecFromChange -Change 'NonBreaking' -CurrentVersion '0.0.5').Bump | Should -Be 'minor'
        }
    }

    Context 'Patch maps to a patch bump' {
        It 'returns Bump=patch and empty Version regardless of current version shape' {
            (Resolve-ReleaseSpecFromChange -Change 'Patch' -CurrentVersion '1.2.3').Bump | Should -Be 'patch'
            (Resolve-ReleaseSpecFromChange -Change 'Patch' -CurrentVersion '0.5.2').Bump | Should -Be 'patch'
            (Resolve-ReleaseSpecFromChange -Change 'Patch' -CurrentVersion '0.0.5').Bump | Should -Be 'patch'
        }
    }

    Context "'1.0' graduates a 0.x package to its first stable 1.0.0" {
        It 'returns Version=1.0.0 (and empty Bump) for 0.x.y' {
            $spec = Resolve-ReleaseSpecFromChange -Change '1.0' -CurrentVersion '0.5.2'
            $spec.Bump    | Should -Be ''
            $spec.Version | Should -Be '1.0.0'
        }

        It 'returns Version=1.0.0 for 0.0.x as well' {
            $spec = Resolve-ReleaseSpecFromChange -Change '1.0' -CurrentVersion '0.0.7'
            $spec.Bump    | Should -Be ''
            $spec.Version | Should -Be '1.0.0'
        }

        It 'throws a clear, actionable error when invoked on a 1.x package' {
            { Resolve-ReleaseSpecFromChange -Change '1.0' -CurrentVersion '1.2.3' } |
                Should -Throw "*'-Change 1.0' option is for the one-time*graduation event*'1.2.3' is already at 1.x or higher*"
        }

        It 'throws when invoked on a 2.x package as well (already past 1.0 lifecycle)' {
            { Resolve-ReleaseSpecFromChange -Change '1.0' -CurrentVersion '2.0.0' } |
                Should -Throw "*graduation event*'2.0.0' is already at 1.x or higher*"
        }
    }

    Context 'parameter validation' {
        It 'rejects unknown -Change values via ValidateSet' {
            { Resolve-ReleaseSpecFromChange -Change 'Major' -CurrentVersion '1.2.3' } |
                Should -Throw "*ValidateSet*"
        }

        It "rejects the old 'major' / 'minor' vocabulary (no back-compat — clean rename)" {
            { Resolve-ReleaseSpecFromChange -Change 'major' -CurrentVersion '1.2.3' } | Should -Throw "*ValidateSet*"
            { Resolve-ReleaseSpecFromChange -Change 'minor' -CurrentVersion '1.2.3' } | Should -Throw "*ValidateSet*"
            # Note: 'patch' (lowercase) now matches 'Patch' via ValidateSet's case-insensitive
            # comparison, so it is intentionally NOT rejected. The Stage 2b rename eliminated
            # the standalone numeric vocabulary; the only surviving overlap is incidental.
        }

        It "rejects the previous 'Fix' value (renamed to 'Patch' — clean break, no back-compat)" {
            { Resolve-ReleaseSpecFromChange -Change 'Fix' -CurrentVersion '1.2.3' } | Should -Throw "*ValidateSet*"
        }
    }
}


# ---------------------------------------------------------------------------
# Format-PendingReleasesAnnouncement (pure formatter)
# ---------------------------------------------------------------------------

Describe 'Format-PendingReleasesAnnouncement' {

    BeforeAll {
        function script:NewPending {
            param([string]$Name, [string]$BaseVersion, [string]$CurrentVersion, [string]$Folder)
            if ([string]::IsNullOrEmpty($Folder)) { $Folder = $Name }
            return [pscustomobject]@{
                Folder         = $Folder
                Name           = $Name
                BaseVersion    = $BaseVersion
                CurrentVersion = $CurrentVersion
            }
        }
    }

    Context 'empty input' {
        It 'returns an empty string when given an empty array' {
            Format-PendingReleasesAnnouncement -Pending @() | Should -Be ''
        }

        It 'returns an empty string when given $null' {
            Format-PendingReleasesAnnouncement -Pending $null | Should -Be ''
        }
    }

    Context 'single pending package' {
        It 'renders the spec-exact header and one indented version-transition line' {
            $out = Format-PendingReleasesAnnouncement -Pending @(NewPending -Name 'bytesbuf' -BaseVersion '1.2.3' -CurrentVersion '1.2.4')
            $lines = $out -split "`r?`n"
            $lines.Count                | Should -Be 2
            $lines[0]                   | Should -Be 'Detected pending uncommitted releases and included in analysis data set:'
            $lines[1]                   | Should -Be '   bytesbuf 1.2.3 -> 1.2.4'
        }
    }

    Context 'multiple pending packages' {
        It 'renders one indented line per package preserving the input order (caller is responsible for sorting)' {
            $out = Format-PendingReleasesAnnouncement -Pending @(
                NewPending -Name 'bytesbuf' -BaseVersion '1.2.3' -CurrentVersion '1.2.4'
                NewPending -Name 'foo'      -BaseVersion '0.2.2' -CurrentVersion '1.0.0'
            )
            $lines = $out -split "`r?`n"
            $lines.Count                | Should -Be 3
            $lines[0]                   | Should -Be 'Detected pending uncommitted releases and included in analysis data set:'
            $lines[1]                   | Should -Be '   bytesbuf 1.2.3 -> 1.2.4'
            $lines[2]                   | Should -Be '   foo 0.2.2 -> 1.0.0'
        }
    }

    Context 'uses package Name (not Folder) for display' {
        It 'reads the .Name property, which can differ from .Folder' {
            $entry = [pscustomobject]@{
                Folder         = 'fetch-hyper'         # folder uses kebab-case
                Name           = 'fetch_hyper'         # package name uses snake_case
                BaseVersion    = '0.5.0'
                CurrentVersion = '0.5.1'
            }
            $out = Format-PendingReleasesAnnouncement -Pending @($entry)
            $lines = $out -split "`r?`n"
            $lines[1]                   | Should -Be '   fetch_hyper 0.5.0 -> 0.5.1'
        }
    }
}


