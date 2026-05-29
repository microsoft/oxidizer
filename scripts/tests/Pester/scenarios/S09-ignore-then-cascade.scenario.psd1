@{
    Name        = 'S09-ignore-then-cascade'
    Description = 'User declines b, then accepts c. Releasing c cascade-bumps b into the release set. The scan reports the override via the "Previously ignored package was cascade-bumped" notice and removes b from the declined set. Validates the ignore-then-cascade handoff path.'

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
            # Iter 0 of the scan: ignore b.
            @{ Match = "Choose option for 'b'"; Reply = '2' }
            # Iter 1: accept c as patch.
            @{ Match = "Choose option for 'c'"; Reply = '5' }
            # No further prompt: b is now in the release set via cascade, not a finding.
        )
    }

    Expect = @{
        # a patch (0.1.0 → 0.1.1).
        # c accepted as patch (0.3.0 → 0.3.1).
        # b cascade-bumped from c (0.2.0 → 0.2.1) despite being previously declined.
        # a cascade from c bullet-only (0.1.1 already >= required).
        Released = @(
            @{ Crate = 'a'; To = '0.1.1' }
            @{ Crate = 'c'; To = '0.3.1' }
            @{ Crate = 'b'; To = '0.2.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
        )
        UnconsumedAnswers = @()
    }
}
