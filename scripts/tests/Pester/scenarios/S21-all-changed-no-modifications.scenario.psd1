@{
    Name        = 'S21-all-changed-no-modifications'
    Description = 'Linear3 with NO modifications. Run in -Mode all-changed; expect the entry point to print "no changed packages detected" and exit cleanly without invoking the review loop or releasing anything.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @()  # no modifications

    Run = @{
        Mode    = 'all-changed'
        Answers = @()  # no prompts expected — early exit before review loop
    }

    Expect = @{
        # No releases, no prompts raised.
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
