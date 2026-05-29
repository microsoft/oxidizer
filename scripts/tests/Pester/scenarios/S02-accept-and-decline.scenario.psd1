@{
    Name        = 'S02-accept-and-decline'
    Description = 'Linear3 with both upstream crates modified: user accepts b (which releases as minor) and declines c. Final release set = a + b; c stays unreleased.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Crate = 'b' }
        @{ Op = 'ModifySource'; Crate = 'c' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        CrateName = 'a'
        Change    = 'Patch'
        BaseRef   = 'HEAD~1'
        Answers   = @(
            @{ Match = "Choose option for 'b'"; Reply = '4' }
            @{ Match = "Choose option for 'c'"; Reply = '2' }
        )
    }

    Expect = @{
        # In 0.x semver convention (per Get-NextVersion), "minor" bump on 0.2.0
        # is patch-style → 0.2.1 (true breaking is "major" → 0.3.0). b's cascade
        # to a requires 0.1.1 which a already satisfies (from the initial patch),
        # so a stays at 0.1.1 (bullet-only).
        Released = @(
            @{ Crate = 'a'; To = '0.1.1' }
            @{ Crate = 'b'; To = '0.2.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
        )
        UnconsumedAnswers = @()
    }
}
