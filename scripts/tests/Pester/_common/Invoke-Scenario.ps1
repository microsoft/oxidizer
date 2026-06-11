# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    End-to-end scenario runner for release-script Pester tests.

.DESCRIPTION
    Loads a PSD1 scenario descriptor, builds a synthetic Cargo workspace,
    replays a history of operations, then invokes Invoke-ReleasePackagesMain
    in-process with mocked Read-Host / Invoke-WorkspaceCheck /
    Test-InteractiveSession. Returns a result object the test can assert on.

    The runner is invoked from Pester It blocks so Mock works correctly.
    The caller is responsible for dot-sourcing scripts/lib/release-flow.ps1
    in a BeforeAll so this runner can refer to Invoke-ReleasePackagesMain and
    Read-Host as known commands.

.PARAMETER ScenarioFile
    Absolute path to a .scenario.psd1 file describing the scenario.

.PARAMETER WorkspaceRoot
    Optional override for where the synthetic workspace is built (defaults
    to a folder derived from the scenario name under $TestDrive when called
    from a Pester test).
#>

function Invoke-Scenario {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$ScenarioFile,
        [string]$WorkspaceRoot
    )

    if (-not (Test-Path $ScenarioFile)) {
        throw "Scenario file not found: $ScenarioFile"
    }

    $scenario = Import-PowerShellDataFile -Path $ScenarioFile
    if (-not $scenario.Name)      { throw "Scenario '$ScenarioFile' missing Name." }
    if (-not $scenario.Workspace) { throw "Scenario '$ScenarioFile' missing Workspace." }
    if (-not $scenario.Run)       { throw "Scenario '$ScenarioFile' missing Run." }

    if (-not $WorkspaceRoot) {
        $WorkspaceRoot = Join-Path $TestDrive ("scn-" + $scenario.Name)
    }

    # --- 1. Build the workspace.
    Reset-ReleaseScriptCaches
    $wsParams = @{ Path = $WorkspaceRoot }
    if ($scenario.Workspace.Preset) { $wsParams.Preset = $scenario.Workspace.Preset }
    if ($scenario.Workspace.Spec)   { $wsParams.Spec   = $scenario.Workspace.Spec }
    $ws = New-SyntheticWorkspace @wsParams

    # --- 2. Replay history.
    foreach ($step in @($scenario.History)) {
        if (-not $step.Op) { throw "Scenario '$($scenario.Name)' has a history step with no Op." }
        switch ($step.Op) {
            'ModifySource'    { $ws.ModifySource($step.Package) }
            'SetVersion'      { $ws.SetVersion($step.Package, $step.To) }
            'SetPublishFalse' { $ws.SetPublishFalse($step.Package) }
            'AddCommit'       { $ws.AddCommit($step.Message) }
            'Commit'          { $ws.AddCommit($step.Message) }
            'EditCargoToml'   {
                # Generic raw text patch on a package's Cargo.toml.
                $cargo = Join-Path $ws.Path "crates\$($step.Package)\Cargo.toml"
                $content = Get-Content $cargo -Raw
                $content = $content -replace $step.Pattern, $step.Replacement
                Set-Content $cargo -Value $content -NoNewline
            }
            default { throw "Scenario '$($scenario.Name)' has unknown history Op '$($step.Op)'." }
        }
    }

    # --- 3. Set up answer queue and prompt-capture state in script scope so
    # the mock script blocks can mutate them.
    $script:ScenarioAnswerQueue = New-Object System.Collections.Queue
    foreach ($a in @($scenario.Run.Answers)) {
        $script:ScenarioAnswerQueue.Enqueue($a)
    }
    $script:ScenarioPromptsRaised = New-Object System.Collections.Generic.List[string]
    $script:ScenarioRepliesGiven  = New-Object System.Collections.Generic.List[string]
    $script:ScenarioSkippedPromptFolders = New-Object System.Collections.Generic.List[string]

    # --- 4. Invoke under mocks. The caller (test) has already mocked the
    # script-level cmdlets by the time this runs. We only invoke the entry
    # point and capture the release records + any thrown exception.
    Push-Location $ws.Path
    try {
        # The scenario harness wires a single entry point — Invoke-ReleasePackagesMain —
        # and selects between its three -Mode values from the scenario PSD1:
        #
        #   Run.Mode = 'changed' → Invoke-ReleasePackagesMain -Mode 'changed'
        #     (interactive guided walk through every modified package; no
        #     -Packages tokens).
        #
        #   Run.Mode = 'all'     → Invoke-ReleasePackagesMain -Mode 'all'
        #     (interactive guided walk through every publishable package, even
        #     ones with no on-disk modifications).
        #
        #   otherwise (default) → Invoke-ReleasePackagesMain -Mode 'targeted'
        #     with explicit -Packages tokens (the historical scenario style).
        $error.Clear()
        $caught = $null

        $runMode = if ($scenario.Run.Mode) { $scenario.Run.Mode } else { 'targeted' }
        # Run.Force is only meaningful in targeted mode — production rejects
        # `-Force` for changed/all because those modes don't accept explicit
        # version pins, so the pin-vs-cascade rejection that -Force overrides
        # cannot fire. Mirror that contract here so scenario PSD1s can't
        # accidentally exercise a code path production also rejects.
        $useForce = [bool]$scenario.Run.Force
        if ($useForce -and $runMode -ne 'targeted') {
            throw "Scenario '$($scenario.Name)' sets Run.Force but Run.Mode='$runMode'; -Force is only valid in targeted mode (it overrides the pin-vs-cascade rejection that only applies to explicit version pins)."
        }
        if ($runMode -in @('changed', 'all')) {
            if ($null -ne $scenario.Run.Packages -or $null -ne $scenario.Run.PackageName) {
                throw "Scenario '$($scenario.Name)' uses Run.Mode='$runMode' but also sets Run.Packages/Run.PackageName; choose one."
            }
            try {
                $releases = Invoke-ReleasePackagesMain -Mode $runMode 6> $null
            } catch {
                $caught = $_
                $releases = @()
            }
        } elseif ($runMode -eq 'targeted') {
            # New-style scenarios provide Run.Packages directly (a string[] of
            # '<name>@<change-spec>' tokens). Legacy scenarios provided
            # Run.PackageName + Run.Change/Run.Version + Run.BaseRef; translate
            # them on the fly so the scenario PSD1s can migrate independently.
            $packageTokens = $null
            if ($null -ne $scenario.Run.Packages -and @($scenario.Run.Packages).Count -gt 0) {
                $packageTokens = @($scenario.Run.Packages)
            } else {
                if (-not $scenario.Run.PackageName) {
                    throw "Scenario '$($scenario.Name)' must provide either Run.Mode='changed'/'all', Run.Packages, or Run.PackageName."
                }
                $changeSpec = if ($scenario.Run.Version) {
                    $scenario.Run.Version
                } elseif ($scenario.Run.Change) {
                    switch ($scenario.Run.Change) {
                        'Breaking'    { 'breaking' }
                        'NonBreaking' { 'nonbreaking' }
                        'Patch'       { 'patch' }
                        '1.0'         { '1.0.0' }
                        default { throw "Scenario '$($scenario.Name)' has unrecognised Run.Change '$($scenario.Run.Change)'." }
                    }
                } else {
                    # Default change type for bare invocations.
                    'nonbreaking'
                }
                $packageTokens = @("$($scenario.Run.PackageName)@$changeSpec")
            }

            try {
                $invokeArgs = @{ Mode = 'targeted'; Packages = $packageTokens }
                if ($useForce) { $invokeArgs.Force = $true }
                $releases = Invoke-ReleasePackagesMain @invokeArgs 6> $null
            } catch {
                $caught = $_
                $releases = @()
            }
        } else {
            throw "Scenario '$($scenario.Name)' has unknown Run.Mode '$runMode'. Expected 'targeted', 'changed', or 'all'."
        }
    } finally {
        Pop-Location
    }

    return [pscustomobject]@{
        Scenario        = $scenario
        Workspace       = $ws
        Releases        = @($releases)
        PromptsRaised   = $script:ScenarioPromptsRaised.ToArray()
        RepliesGiven    = $script:ScenarioRepliesGiven.ToArray()
        SkippedPrompts  = $script:ScenarioSkippedPromptFolders.ToArray()
        UnconsumedAnswers = @($script:ScenarioAnswerQueue.ToArray())
        Error           = $caught
    }
}

# Helper invoked from the Read-Host mock so the answer-matching logic lives in
# one place. Returns the reply string and records the prompt.
function Resolve-ScenarioPromptReply {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$Prompt)

    $script:ScenarioPromptsRaised.Add($Prompt) | Out-Null
    if ($script:ScenarioAnswerQueue.Count -eq 0) {
        throw "Scenario answer queue is empty but prompt arrived: '$Prompt'"
    }
    $next = $script:ScenarioAnswerQueue.Dequeue()
    if ($next.Match -and ($Prompt -notmatch [regex]::Escape($next.Match))) {
        throw "Scenario answer mismatch.`n  Expected to match: $($next.Match)`n  Got prompt:       $Prompt"
    }
    $script:ScenarioRepliesGiven.Add($next.Reply) | Out-Null
    return $next.Reply
}
