@{
    Name        = 'S05-diamond4-aggregation'
    Description = 'Diamond4 (top -> left, right; left, right -> bottom): release top, modify bottom only. Bottom is reachable via two paths (top->left->bottom and top->right->bottom); both chains should be aggregated under a single finding. User accepts, bottom releases with a single release.'

    Workspace = @{ Preset = 'Diamond4' }

    History = @(
        @{ Op = 'ModifySource'; Package = 'bottom' }
        @{ Op = 'AddCommit';    Message = 'bottom edits' }
    )

    Run = @{
        PackageName = 'top'
        Change    = 'Patch'
        BaseRef   = 'HEAD~1'
        Answers   = @(
            # On 0.x.y the menu hides option 5 (patch) because it would be numerically
            # identical to option 4 (non-breaking change), so we pick '4' to drive
            # the same 0.x.y -> 0.x.(y+1) increment.
            @{ Match = "Choose option for 'bottom'"; Reply = '4' }
        )
    }

    Expect = @{
        # top is released per request, bottom is released via the prompt, and bottom's
        # cascade pulls in its dependents (left, right). top is in release set already.
        Released = @(
            @{ Package = 'top';    To = '0.1.1' }
            @{ Package = 'bottom'; To = '0.4.1' }
            @{ Package = 'left';   To = '0.2.1' }
            @{ Package = 'right';  To = '0.3.1' }
        )
        PromptsRaised = @(
            "Choose option for 'bottom'"
        )
        UnconsumedAnswers = @()
    }
}
