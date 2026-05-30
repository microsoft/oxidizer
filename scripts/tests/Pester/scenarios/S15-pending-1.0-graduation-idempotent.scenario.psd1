@{
    Name        = 'S15-pending-1.0-graduation-idempotent'
    Description = 'Re-invocation of `-Change 1.0` on a package whose prior `-Change 1.0` run already graduated it from 0.x to 1.0.0 (pending). The on-disk version is now 1.0.0, but Resolve-ReleaseSpecFromChange would normally throw "already at 1.x" if it inspected on-disk. The cross-invocation logic must instead consult the BASE version (still 0.x at the base ref), translate -Change 1.0 → -Version 1.0.0, and idempotently no-op against the current pending 1.0.0.'

    Workspace = @{
        Preset = 'Linear3'
    }

    History = @(
        # Prior `release-crate.ps1 -Change 1.0` on b graduated it to 1.0.0.
        @{ Op = 'SetVersion'; Package = 'b'; To = '1.0.0' }
    )

    Run = @{
        # Re-invoke with the same -Change 1.0. Must NOT throw the on-disk
        # "already at 1.x" error; must idempotently no-op.
        PackageName = 'b'
        Change    = '1.0'
        BaseRef   = 'HEAD'
        Answers   = @()
    }

    Expect = @{
        # b stays at 1.0.0 (no-op). Cascade uses the effective change (major
        # on 0.2.0 → 1.0.0 is a breaking change at the 0→1 boundary), so a
        # cascades as major. On 0.x.y a's breaking change is 0.1.0 → 0.2.0.
        Released = @(
            @{ Package = 'b'; To = '1.0.0' }
            @{ Package = 'a'; To = '0.2.0' }
        )
        UnconsumedAnswers = @()
    }
}
