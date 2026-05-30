@{
    Name        = 'S09-ignore-then-cascade'
    Description = 'User declines b, then accepts c. Releasing c cascade-releases b into the release set. The scan reports the override via the "Previously ignored package was cascade-released" notice and removes b from the declined set. Per Invariant B, b (now in the release set with a non-breaking cascade-applied change type AND modifications from earlier commits) is RE-SURFACED for elevation review on the next iteration — accepting c does NOT pre-mark b as reviewed. User confirms the cascade-applied change type is sufficient by picking ignore again.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'ModifySource'; Package = 'c' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        PackageName = 'a'
        Change    = 'Patch'
        BaseRef   = 'HEAD~1'
        Answers   = @(
            # Iter 0 of the scan: ignore b.
            @{ Match = "Choose option for 'b'"; Reply = '2' }
            # Iter 1: accept c via option 4 (non-breaking). Option 5 (patch) is hidden
            # on 0.x.y because it would produce the same numeric increment.
            @{ Match = "Choose option for 'c'"; Reply = '4' }
            # Iter 2: c's cascade pulled b into the release set with a
            # non-breaking change (0.2.0 → 0.2.1). Because b also has
            # pre-existing modifications, Invariant B re-surfaces it for
            # elevation review. User picks ignore — the cascade-applied
            # change is fine.
            @{ Match = "Choose option for 'b'"; Reply = '2' }
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
            "Choose option for 'b'"
        )
        UnconsumedAnswers = @()
    }
}
