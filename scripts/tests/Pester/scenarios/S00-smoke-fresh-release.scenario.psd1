@{
    Name        = 'S00-smoke-fresh-release-no-cascade'
    Description = 'Releasing a leaf package with no dependents and no upstream modifications produces a single release record and raises no prompts. Smoke test for the scenario runner.'

    Workspace = @{ Preset = 'Linear2' }   # downstream -> upstream

    History = @(
        # No modifications: the post-release scan should have nothing to report.
    )

    Run = @{
        # 'downstream' has no dependents, so no cascade. Upstream is clean.
        Packages = @('downstream@patch')
        Answers   = @()
    }

    Expect = @{
        Released = @(
            @{ Package = 'downstream'; To = '0.1.1' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
