@{
    Name        = 'S04-non-interactive-skips-prompts'
    Description = 'NonInteractive run with modified upstream packages: post-release scan reports the findings as a warning and exits without prompting; release set stays as originally requested.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'ModifySource'; Package = 'c' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        PackageName      = 'a'
        Change         = 'Patch'
        BaseRef        = 'HEAD~1'
        NonInteractive = $true
        Answers        = @()
    }

    Expect = @{
        Released = @(
            @{ Package = 'a'; To = '0.1.1' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
