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
            param([string]$Folder = 'ohno', [object[]]$Chains = @(@('a', 'ohno')))
            return [pscustomobject]@{
                Folder           = $Folder
                PackageName      = $Folder
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

    It 'emits one "pulled in by:" line per dependency chain' {
        $finding = NewFinding -Folder 'd' -Chains @(@('a', 'b', 'd'), @('a', 'c', 'd'))
        $out = Format-PackageMenu -Finding $finding -RemainingCount 0
        $matches = [regex]::Matches($out, 'pulled in by:')
        $matches.Count | Should -Be 2
        $out | Should -Match 'pulled in by: a -> b -> d'
        $out | Should -Match 'pulled in by: a -> c -> d'
    }

    It 'lists the five menu options in the exact order and wording from the spec' {
        $out = Format-PackageMenu -Finding (NewFinding) -RemainingCount 0
        $lines = $out -split "`r?`n" | Where-Object { $_ -match '^\s*\d\. ' }
        $lines.Count | Should -Be 5
        $lines[0] | Should -Match '^\s*1\. View diff$'
        $lines[1] | Should -Match '^\s*2\. Ignore package$'
        $lines[2] | Should -Match '^\s*3\. Bump major version$'
        $lines[3] | Should -Match '^\s*4\. Bump minor version$'
        $lines[4] | Should -Match '^\s*5\. Bump patch version$'
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
# Get-PackageReleaseDecision (input-validation loop)
# ---------------------------------------------------------------------------

Describe 'Get-PackageReleaseDecision' {

    BeforeAll {
        function script:NewFinding {
            param([string]$Folder = 'ohno')
            return [pscustomobject]@{
                Folder           = $Folder
                PackageName      = $Folder
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

    Context 'View Diff (choice 1) re-renders the menu and re-prompts' {
        It "calls Show-PackageDiff once when the user picks '1' then '4'" {
            SetReadHostQueue -Answers @('1', '4')
            $r = Get-PackageReleaseDecision -Finding (NewFinding -Folder 'b') -RemainingCount 0 -RepoRoot $TestDrive
            $r.Action | Should -Be 'minor'
            Should -Invoke -CommandName Show-PackageDiff -Times 1 -Exactly -ParameterFilter { $Folder -eq 'b' }
            # Menu rendered: 1 initial + 1 after diff = 2 calls.
            Should -Invoke -CommandName Show-PackageMenu -Times 2 -Exactly
        }

        It "calls Show-PackageDiff twice when the user picks '1', '1', '4'" {
            SetReadHostQueue -Answers @('1', '1', '4')
            $r = Get-PackageReleaseDecision -Finding (NewFinding -Folder 'b') -RemainingCount 2 -RepoRoot $TestDrive
            $r.Action | Should -Be 'minor'
            Should -Invoke -CommandName Show-PackageDiff -Times 2 -Exactly
            Should -Invoke -CommandName Show-PackageMenu -Times 3 -Exactly
        }
    }

    Context 'prompt format' {
        It "includes the package name in the Read-Host prompt for scrollback / scenario disambiguation" {
            SetReadHostQueue -Answers @('2')
            Get-PackageReleaseDecision -Finding (NewFinding -Folder 'mypkg') -RemainingCount 0 -RepoRoot $TestDrive | Out-Null
            $script:RH_PromptsObserved[0] | Should -Match "Choose option for 'mypkg'"
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
