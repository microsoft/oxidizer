@{
    Name        = 'S13-pending-primary-noop-explicit-version'
    Description = 'Idempotent re-invocation with the same explicit -Version equal to the pending current version. Equal is a no-op (the prior run already produced this version). Counterpart to S14 which uses -Change instead of -Version.'

    # Stable 1.x topology so -Version semantics are unambiguous (0.x.y collapses
    # several change types onto the same numeric increment).
    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
    }

    History = @(
        # Prior `release-crate.ps1 -Version 1.2.4` left upstream pending.
        @{ Op = 'SetVersion'; Package = 'upstream'; To = '1.2.4' }
    )

    Run = @{
        # Re-invoke with the EXACT same explicit -Version. Must idempotently no-op.
        PackageName = 'upstream'
        Version   = '1.2.4'
        BaseRef   = 'HEAD'
        Answers   = @()
    }

    Expect = @{
        # upstream stays at 1.2.4 (no-op). downstream cascades from the EFFECTIVE
        # base→current change type (patch on 1.2.3 → 1.2.4) and itself doesn't
        # expose upstream, so its cascade-applied change type is patch → 1.0.1.
        Released = @(
            @{ Package = 'upstream';   To = '1.2.4' }
            @{ Package = 'downstream'; To = '1.0.1' }
        )
        UnconsumedAnswers = @()
    }
}
