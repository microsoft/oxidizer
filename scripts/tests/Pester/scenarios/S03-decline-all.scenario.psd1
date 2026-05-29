@{
    Name        = 'S03-decline-all'
    Description = 'Linear3 with both upstream crates modified: user declines both. Final release is the originally requested crate only; both upstream findings stay unreleased.'

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
            @{ Match = "Choose option for 'b'"; Reply = '2' }
            @{ Match = "Choose option for 'c'"; Reply = '2' }
        )
    }

    Expect = @{
        Released = @(
            @{ Crate = 'a'; To = '0.1.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
        )
        UnconsumedAnswers = @()
    }
}
