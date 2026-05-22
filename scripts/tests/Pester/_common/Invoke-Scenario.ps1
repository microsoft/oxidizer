# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    End-to-end scenario runner for release-script Pester tests.

.DESCRIPTION
    Loads a PSD1 scenario descriptor, builds a synthetic Cargo workspace,
    replays a history of operations, then invokes Invoke-ReleaseMain
    in-process with mocked Read-Host / Invoke-WorkspaceCheck /
    Test-InteractiveSession. Returns a result object the test can assert on.

    The runner is invoked from Pester It blocks so Mock works correctly.
    The caller is responsible for dot-sourcing release-crate.ps1 in a
    BeforeAll (with $env:OXI_RELEASE_CRATE_NOEXEC = '1') so this runner can
    refer to Invoke-ReleaseMain and Read-Host as known commands.

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
    Invalidate-WorkspaceMetadataCache
    $wsParams = @{ Path = $WorkspaceRoot }
    if ($scenario.Workspace.Preset) { $wsParams.Preset = $scenario.Workspace.Preset }
    if ($scenario.Workspace.Spec)   { $wsParams.Spec   = $scenario.Workspace.Spec }
    $ws = New-SyntheticWorkspace @wsParams

    # --- 2. Replay history.
    foreach ($step in @($scenario.History)) {
        if (-not $step.Op) { throw "Scenario '$($scenario.Name)' has a history step with no Op." }
        switch ($step.Op) {
            'ModifySource'    { $ws.ModifySource($step.Crate) }
            'BumpVersion'     { $ws.BumpVersion($step.Crate, $step.To) }
            'SetPublishFalse' { $ws.SetPublishFalse($step.Crate) }
            'AddCommit'       { $ws.AddCommit($step.Message) }
            'Commit'          { $ws.AddCommit($step.Message) }
            'EditCargoToml'   {
                # Generic raw text patch on a crate's Cargo.toml.
                $cargo = Join-Path $ws.Path "crates\$($step.Crate)\Cargo.toml"
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
        $runArgs = @{
            CrateName = $scenario.Run.CrateName
            BaseRef   = $(if ($scenario.Run.BaseRef) { $scenario.Run.BaseRef } else { 'HEAD~1' })
        }
        if ($scenario.Run.Bump)           { $runArgs.Bump    = $scenario.Run.Bump }
        if ($scenario.Run.Version)        { $runArgs.Version = $scenario.Run.Version }
        if ($scenario.Run.NonInteractive) { $runArgs.NonInteractive = $true }

        $error.Clear()
        $caught = $null
        try {
            $releases = Invoke-ReleaseMain @runArgs 6> $null
        } catch {
            $caught = $_
            $releases = @()
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
