@{
    Name        = 'S07-view-diff-then-decide'
    Description = 'User exercises the View Diff option (menu choice 1) for a finding before deciding. After viewing the diff, the menu re-renders and the user picks minor (option 4) for b, then ignores c. Validates that choice 1 re-prompts on the same package rather than advancing.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Crate = 'b' }
        @{ Op = 'ModifySource'; Crate = 'c' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        CrateName = 'a'
        Bump      = 'patch'
        BaseRef   = 'HEAD~1'
        Answers   = @(
            # First prompt for b: view the diff.
            @{ Match = "Choose option for 'b'"; Reply = '1' }
            # Menu re-renders for b after diff; this time choose minor.
            @{ Match = "Choose option for 'b'"; Reply = '4' }
            # Next iteration prompts for c; user ignores.
            @{ Match = "Choose option for 'c'"; Reply = '2' }
        )
    }

    Expect = @{
        # b accepted as minor (0.x: patch-style) → 0.2.1. a's cascade bullet-only at 0.1.1.
        # c is declined; no entry in releases.
        Released = @(
            @{ Crate = 'a'; To = '0.1.1' }
            @{ Crate = 'b'; To = '0.2.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'b'"
            "Choose option for 'c'"
        )
        UnconsumedAnswers = @()
    }
}
