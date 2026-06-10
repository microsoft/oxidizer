# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S00-smoke-fresh-release'
    Description = 'No-cascade smoke test: releasing a leaf package with no dependents and no dependency modifications produces a single release record and raises no prompts. Smoke test for the scenario runner.'

    Workspace = @{ Preset = 'Linear2' }   # dependent -> dependency

    History = @(
        # No modifications: the post-release scan should have nothing to report.
    )

    Run = @{
        # 'dependent' has no dependents, so no cascade. Dependency is clean.
        Packages = @('dependent@patch')
        Answers   = @()
    }

    Expect = @{
        Released = @(
            @{ Package = 'dependent'; To = '0.1.1' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
