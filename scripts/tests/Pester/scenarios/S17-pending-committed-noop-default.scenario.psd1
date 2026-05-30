@{
    Name        = 'S17-pending-committed-noop-default'
    Description = 'Pending-release detection is committed-vs-uncommitted agnostic: same behaviour as S12 but the prior `release-crate.ps1` run has been committed (not just staged). BaseRef defaults to HEAD~1 so the committed bump is in-branch but not in the base ref. The script must detect `b` as pending, skip the primary change (instead of double-incrementing to 0.2.2), and still run the dependent cascade idempotently.'

    Workspace = @{
        Preset = 'Linear3'
    }

    History = @(
        # Prior `release-crate.ps1` run committed in this branch.
        @{ Op = 'SetVersion'; Package = 'b'; To = '0.2.1' }
        @{ Op = 'AddCommit';  Message = 'feat(b): release v0.2.1' }
    )

    Run = @{
        # Re-invoke for the same pending package without explicit -Change.
        PackageName = 'b'
        # BaseRef defaults to HEAD~1, which points at the baseline commit
        # (before the SetVersion+commit step above), so the committed bump
        # is correctly recognised as in-branch / pending.
        Answers   = @()
    }

    Expect = @{
        # Identical to S12: b stays at 0.2.1 (no double-increment); a is the
        # dependent and gets patched.
        Released = @(
            @{ Package = 'b'; To = '0.2.1' }
            @{ Package = 'a'; To = '0.1.1' }
        )
        UnconsumedAnswers = @()
    }
}
