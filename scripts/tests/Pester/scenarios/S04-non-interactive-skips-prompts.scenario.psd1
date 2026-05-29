@{
    Name        = 'S04-non-interactive-skips-prompts'
    Description = 'NonInteractive run with modified upstream crates: post-release scan reports the findings as a warning and exits without prompting; release set stays as originally requested.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Crate = 'b' }
        @{ Op = 'ModifySource'; Crate = 'c' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        CrateName      = 'a'
        Change         = 'Patch'
        BaseRef        = 'HEAD~1'
        NonInteractive = $true
        Answers        = @()
    }

    Expect = @{
        Released = @(
            @{ Crate = 'a'; To = '0.1.1' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
