@{
    Name        = 'S11-stable-version-minor-distinct'
    Description = 'Companion to S10: same stable workspace, but the user picks option 4 (non-breaking) on upstream. Verifies upstream ends at 1.3.0 (a minor bump), confirming that on >=1.x.y packages options 4 and 5 resolve to genuinely different on-disk versions.'

    Workspace = @{
        Spec = @{
            Crates = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
    }

    History = @(
        @{ Op = 'ModifySource'; Crate = 'upstream' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        CrateName = 'downstream'
        Change    = 'Patch'
        BaseRef   = 'HEAD~1'
        Answers   = @(
            # On a stable >=1.x.y package the menu offers [1-5]; '4' selects the
            # minor (non-breaking) path, distinct from the patch path of option 5.
            @{ Match = "Choose option for 'upstream'"; Reply = '4' }
        )
    }

    Expect = @{
        # downstream: 1.0.0 -> 1.1.0. The user requested -Change Patch, but the
        # post-release scan accepts upstream as a *minor* bump (1.2.3 -> 1.3.0).
        # The cascade then escalates downstream to a minor bump too — on stable
        # >=1.x.y, a non-breaking-but-minor-class change in an upstream propagates
        # as minor in dependents (see Test-IsBreakingChange + the exposing-cascade
        # logic in Invoke-ReleaseFlow).
        # upstream: 1.2.3 -> 1.3.0 (minor; option 5 would have given 1.2.4).
        Released = @(
            @{ Crate = 'downstream'; To = '1.1.0' }
            @{ Crate = 'upstream';   To = '1.3.0' }
        )
        PromptsRaised = @(
            "Choose option for 'upstream' [1-5]"
        )
        UnconsumedAnswers = @()
    }
}
