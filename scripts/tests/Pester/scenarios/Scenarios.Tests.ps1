# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Phase 6 — end-to-end scenario tests.

.DESCRIPTION
    Loads every *.scenario.psd1 under scripts/tests/Pester/scenarios/ and
    runs each one via Invoke-Scenario. Each scenario builds a synthetic
    workspace, replays history, invokes Invoke-ReleasePackagesMain in-process
    (with mocked Read-Host / Invoke-WorkspaceCheck / Test-InteractiveSession),
    then asserts on the resulting release records and raised prompts.
#>

BeforeAll {
    . (Join-Path $PSScriptRoot '..\_common\TestHelpers.ps1')
    . (Join-Path $PSScriptRoot '..\_common\New-SyntheticWorkspace.ps1')
    . (Join-Path $PSScriptRoot '..\_common\Invoke-Scenario.ps1')
    . (Join-Path (Get-OxiRepoRoot) 'scripts\lib\release-flow.ps1')
}

# Discover all scenarios at top-level (Discovery phase) so the cached list is
# available to both `BeforeAll` (Run phase) and the `It -ForEach` parameter
# (also evaluated at Discovery time). Computing it once here keeps the source
# of truth in a single place.
$script:ScenarioFiles = Get-ChildItem -Path (Join-Path $PSScriptRoot '..\scenarios') -Filter '*.scenario.psd1' |
    ForEach-Object { @{ File = $_.FullName; Name = $_.BaseName -replace '\.scenario$', '' } }

Describe 'End-to-end release scenarios' {

    # Per-It mocks rather than BeforeAll mocks: Pester 5 requires Mocks inside
    # the It or in a BeforeEach to take effect for the It's invocation.
    BeforeEach {
        # cargo check is too expensive (and irrelevant) for scenarios; mock it to no-op.
        Mock -CommandName Invoke-WorkspaceCheck -MockWith { } -Verifiable:$false

        # Force interactive mode so the prompt flow is exercised even when
        # tests run under CI / pwsh non-tty.
        Mock -CommandName Test-InteractiveSession -MockWith { $true } -Verifiable:$false

        # The entry point's pre-flight asserts git and cargo-semver-checks are on
        # PATH. Scenarios mock the classifier (Get-CrateRequiredChangeType) so the
        # real cargo-semver-checks binary is never invoked and need not be
        # installed on the runner — satisfy the presence check via mock.
        Mock -CommandName Test-CommandExists -MockWith { $true } -Verifiable:$false

        # Suppress real editor launches when scenarios exercise the View Diff path.
        Mock -CommandName Open-PathWithPreferredEditor -MockWith { } -Verifiable:$false

        # Route Read-Host through the scenario answer queue.
        Mock -CommandName Read-Host -MockWith {
            param([string]$Prompt)
            return Resolve-ScenarioPromptReply -Prompt $Prompt
        } -Verifiable:$false

        # Replace real cargo-semver-checks with the scenario's simulated verdict
        # map (folder -> change type), so cascade/self-floor classification is
        # deterministic and offline. Unmapped folders default to 'none'.
        Mock -CommandName Get-CrateRequiredChangeType -MockWith {
            param([string]$Folder, [string]$CargoName, [string]$RepoRoot)
            if ($script:ScenarioSemverVerdicts -and $script:ScenarioSemverVerdicts.ContainsKey($Folder)) {
                return $script:ScenarioSemverVerdicts[$Folder]
            }
            return 'none'
        } -Verifiable:$false
    }

    It '<Name>' -ForEach $script:ScenarioFiles {
        $result = Invoke-Scenario -ScenarioFile $File
        $expect = $result.Scenario.Expect

        if ($expect.Throws) {
            $result.Error | Should -Not -BeNullOrEmpty -Because "scenario expected an exception"
            if ($expect.ThrowsMatches) {
                $result.Error.Exception.Message | Should -Match ([regex]::Escape($expect.ThrowsMatches)) -Because "exception message did not contain the expected substring"
            }
        } elseif ($result.Error) {
            throw "Scenario '$Name' threw: $($result.Error)"
        }

        # --- Released packages: at least every expected entry must appear with the expected version.
        # Use a $null-check (not truthiness) so a scenario that explicitly
        # asserts NO releases via `Released = @()` still triggers the
        # release-set bound check below — an empty array is falsy in
        # PowerShell, and the truthiness form would have skipped the entire
        # block and silently let unexpected releases leak through.
        if ($null -ne $expect.Released) {
            $diag = "PromptsRaised: [$($result.PromptsRaised -join ' | ')]; RepliesGiven: [$($result.RepliesGiven -join ' | ')]; Releases: [$(($result.Releases | ForEach-Object { "$($_.Package)=$($_.NewVersion)" }) -join ', ')]"
            foreach ($exp in @($expect.Released)) {
                $actual = $result.Releases | Where-Object { $_.Package -eq $exp.Package } | Select-Object -First 1
                $actual | Should -Not -BeNullOrEmpty -Because "scenario expected '$($exp.Package)' in the release set; $diag"
                $actual.NewVersion | Should -Be $exp.To -Because "scenario expected '$($exp.Package)' to end at $($exp.To); $diag"
            }
            # Bound the release set: no extra packages beyond those expected.
            $expectedNames = @($expect.Released | ForEach-Object { $_.Package })
            $actualNames = @($result.Releases | ForEach-Object { $_.Package })
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
