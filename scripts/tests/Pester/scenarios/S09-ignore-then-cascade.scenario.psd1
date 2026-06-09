# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S09-ignore-then-cascade'
    Description = 'User declines b, then accepts c. Releasing c cascade-releases b into the release set at a non-breaking level. Because decisions are final, the planner silently accepts the cascade-applied level for b without re-prompting — the user already expressed their preference not to elevate. The cascade reason for b is surfaced in the final Show-ReleasePlan output for transparency. Confirms the simplified semantics: each package is prompted at most once.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Package = 'a' }
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'ModifySource'; Package = 'c' }
        @{ Op = 'AddCommit';    Message = 'dependency edits' }
    )

    Run = @{
        Packages = @('a@patch')
        Answers   = @(
            # Iter 0 of the scan: ignore b.
            @{ Match = "Choose option for 'b'"; Reply = '2' } # Skip
            # Iter 1: accept c via option 4 (non-breaking). Option 5 (patch) is hidden
            # on 0.x.y because it would produce the same numeric increment.
            # c's cascade then pulls b into the release set at non-breaking
            # (0.2.0 → 0.2.1). Because b was previously declined, the planner
            # silently accepts the cascade-applied level and does NOT re-prompt.
            @{ Match = "Choose option for 'c'"; Reply = '4' } # Non-breaking
        )
    }

    Expect = @{
        # a patch (0.1.0 → 0.1.1).
        # c accepted as patch (0.3.0 → 0.3.1).
        # b cascade-released from c (0.2.0 → 0.2.1) despite being previously declined.
        # a cascade from c bullet-only (0.1.1 already >= required).
        Released = @(
            @{ Package = 'a'; To = '0.1.1' }
            @{ Package = 'c'; To = '0.3.1' }
            @{ Package = 'b'; To = '0.2.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
        )
        UnconsumedAnswers = @()
    }
}
