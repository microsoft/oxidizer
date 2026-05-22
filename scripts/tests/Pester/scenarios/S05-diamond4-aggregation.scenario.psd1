@{
    Name        = 'S05-diamond4-aggregation'
    Description = 'Diamond4 (top -> left, right; left, right -> bottom): release top, modify bottom only. Bottom is reachable via two paths (top->left->bottom and top->right->bottom); both chains should be aggregated under a single finding. User accepts, bottom releases with a single bump.'

    Workspace = @{ Preset = 'Diamond4' }

    History = @(
        @{ Op = 'ModifySource'; Crate = 'bottom' }
        @{ Op = 'AddCommit';    Message = 'bottom edits' }
    )

    Run = @{
        CrateName = 'top'
        Bump      = 'patch'
        BaseRef   = 'HEAD~1'
        Answers   = @(
            @{ Match = "Release 'bottom' too"; Reply = 'y' }
            @{ Match = "Bump kind for 'bottom'"; Reply = 'p' }
        )
    }

    Expect = @{
        # top is bumped per request, bottom is released via the prompt, and bottom's
        # cascade pulls in its dependents (left, right). top is in release set already.
        Released = @(
            @{ Crate = 'top';    To = '0.1.1' }
            @{ Crate = 'bottom'; To = '0.4.1' }
            @{ Crate = 'left';   To = '0.2.1' }
            @{ Crate = 'right';  To = '0.3.1' }
        )
        PromptsRaised = @(
            "Release 'bottom' too"
            "Bump kind for 'bottom'"
        )
        UnconsumedAnswers = @()
    }
}
