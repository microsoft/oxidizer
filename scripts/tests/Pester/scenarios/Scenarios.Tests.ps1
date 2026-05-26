# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Phase 6 — end-to-end scenario tests.

.DESCRIPTION
    Loads every *.scenario.psd1 under scripts/tests/Pester/scenarios/ and
    runs each one via Invoke-Scenario. Each scenario builds a synthetic
    workspace, replays history, invokes Invoke-ReleaseMain in-process
    (with mocked Read-Host / Invoke-WorkspaceCheck / Test-InteractiveSession),
    then asserts on the resulting release records and raised prompts.
#>

BeforeAll {
    . (Join-Path $env:OXI_TEST_COMMON 'TestHelpers.ps1')
    . (Join-Path $env:OXI_TEST_COMMON 'New-SyntheticWorkspace.ps1')
    . (Join-Path $env:OXI_TEST_COMMON 'Invoke-Scenario.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')

    # Discover all scenarios.
    $script:ScenarioFiles = Get-ChildItem -Path (Join-Path $PSScriptRoot '..\scenarios') -Filter '*.scenario.psd1' |
        ForEach-Object { @{ File = $_.FullName; Name = $_.BaseName -replace '\.scenario$', '' } }
}

Describe 'End-to-end release scenarios' {

    # Per-It mocks rather than BeforeAll mocks: Pester 5 requires Mocks inside
    # the It or in a BeforeEach to take effect for the It's invocation.
    BeforeEach {
        # cargo check is too expensive (and irrelevant) for scenarios; mock it to no-op.
        Mock -CommandName Invoke-WorkspaceCheck -MockWith { } -Verifiable:$false

        # Force interactive mode so the prompt flow is exercised even when
        # tests run under CI / pwsh non-tty.
        Mock -CommandName Test-InteractiveSession -MockWith { $true } -Verifiable:$false

        # Route Read-Host through the scenario answer queue.
        Mock -CommandName Read-Host -MockWith {
            param([string]$Prompt)
            return Resolve-ScenarioPromptReply -Prompt $Prompt
        } -Verifiable:$false
    }

    It '<Name>' -ForEach @(
        Get-ChildItem -Path (Join-Path $PSScriptRoot '..\scenarios') -Filter '*.scenario.psd1' |
            ForEach-Object { @{ File = $_.FullName; Name = $_.BaseName -replace '\.scenario$', '' } }
    ) {
        $result = Invoke-Scenario -ScenarioFile $File
        $expect = $result.Scenario.Expect

        if ($result.Error) {
            throw "Scenario '$Name' threw: $($result.Error)"
        }

        # --- Released crates: at least every expected entry must appear with the expected version.
        if ($expect.Released) {
            $diag = "PromptsRaised: [$($result.PromptsRaised -join ' | ')]; RepliesGiven: [$($result.RepliesGiven -join ' | ')]; Releases: [$(($result.Releases | ForEach-Object { "$($_.Crate)=$($_.NewVersion)" }) -join ', ')]"
            foreach ($exp in @($expect.Released)) {
                $actual = $result.Releases | Where-Object { $_.Crate -eq $exp.Crate } | Select-Object -First 1
                $actual | Should -Not -BeNullOrEmpty -Because "scenario expected '$($exp.Crate)' in the release set; $diag"
                $actual.NewVersion | Should -Be $exp.To -Because "scenario expected '$($exp.Crate)' to end at $($exp.To); $diag"
            }
            # Bound the release set: no extra crates beyond those expected.
            $expectedNames = @($expect.Released | ForEach-Object { $_.Crate })
            $actualNames = @($result.Releases | ForEach-Object { $_.Crate })
            $unexpected = $actualNames | Where-Object { $expectedNames -notcontains $_ }
            $unexpected | Should -BeNullOrEmpty -Because "scenario expected only [$($expectedNames -join ', ')]; got extras: [$($unexpected -join ', ')]"
        }

        # --- Prompts raised (substring match, ordered).
        if ($null -ne $expect.PromptsRaised) {
            $expectedPrompts = @($expect.PromptsRaised)
            $actualPrompts = @($result.PromptsRaised)
            $actualPrompts.Count | Should -Be $expectedPrompts.Count -Because "expected $($expectedPrompts.Count) prompts; got $($actualPrompts.Count): [$($actualPrompts -join ' | ')]"
            for ($i = 0; $i -lt $expectedPrompts.Count; $i++) {
                $actualPrompts[$i] | Should -Match ([regex]::Escape($expectedPrompts[$i])) -Because "prompt #$i did not match"
            }
        }

        # --- All scripted answers must have been consumed.
        if ($null -ne $expect.UnconsumedAnswers) {
            $result.UnconsumedAnswers.Count | Should -Be ($expect.UnconsumedAnswers | Measure-Object).Count -Because "unconsumed answers remain: $($result.UnconsumedAnswers | ConvertTo-Json -Compress)"
        }
    }
}
