# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S16-stable-cascade-elevation'
    Description = 'Stable 1.x topology exercising Invariant B end-to-end. Three-package chain ''top -> middle -> bottom'' where ''middle'' has pre-existing source modifications. User releases ''bottom'' as patch. Cascade pulls ''middle'' (1.0.0 -> 1.0.1) and ''top'' (1.0.0 -> 1.0.1) as patch changes. Because ''middle'' is ALSO modified and its cascade-applied change type is below breaking, the post-release scan surfaces ''middle'' (reached via ''top.Deps = [middle]''). User picks option 4 (non-breaking) → ''middle'' escalates from cascade-applied 1.0.1 to 1.1.0, and the re-cascade lifts ''top'' to 1.1.0 too (exposing-dependent minor cascade).

This is the stable-version companion to S06 (the same flow on 0.x.y). Validates that Invariant B elevation works correctly through Invoke-ReleaseFlow''s ``$isPendingPrimary`` branch on >=1.x packages.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'top';    Version = '1.0.0'; Deps = @(@{ Name = 'middle' }) }
                @{ Name = 'middle'; Version = '1.0.0'; Deps = @(@{ Name = 'bottom' }) }
                @{ Name = 'bottom'; Version = '1.0.0' }
            )
        }
    }

    History = @(
        @{ Op = 'ModifySource'; Package = 'middle' }
        @{ Op = 'AddCommit';    Message = 'middle source edits' }
    )

    Run = @{
        Packages = @('bottom@patch')
        Answers   = @(
            # Invariant B: middle was cascade-pulled with a patch change
            # AND has pre-existing modifications. User elevates to
            # non-breaking (option 4 = minor on 1.x).
            @{ Match = "Choose option for 'middle'"; Reply = '4' }
        )
    }

    Expect = @{
        # bottom: 1.0.0 -> 1.0.1 (user-requested patch).
        # middle: cascade-released to 1.0.1, then escalated to 1.1.0 by the
        #         post-release scan accepting it as non-breaking.
        # top:    cascade-released to 1.0.1 originally, then re-cascade-pulled
        #         to 1.1.0 because the exposing-cascade rule lifts dependents
        #         in lock-step with middle's non-breaking change.
        Released = @(
            @{ Package = 'bottom'; To = '1.0.1' }
            @{ Package = 'middle'; To = '1.1.0' }
            @{ Package = 'top';    To = '1.1.0' }
        )
        PromptsRaised = @(
            "Choose option for 'middle' [1-5]"
        )
        UnconsumedAnswers = @()
    }
}
