@{
    Name        = 'S01-clean-upstream-no-prompts'
    Description = 'Linear3 with no upstream modifications produces a clean release: only the released package appears, and no prompts are raised.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @()

    Run = @{
        PackageName = 'a'
        Change    = 'Patch'
        BaseRef   = 'HEAD'
        Answers   = @()
    }

    Expect = @{
        Released = @(
            @{ Package = 'a'; To = '0.1.1' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
