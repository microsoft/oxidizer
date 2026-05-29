@{
    Name        = 'S12-pending-primary-noop-default'
    Description = 'Cross-invocation pending-release detection: the user has already run `release-crate.ps1` on package `b` in a prior invocation (so its Cargo.toml is at 0.2.1 uncommitted), then re-invokes for `b` again. The script must detect `b` as pending, skip the primary bump (instead of double-bumping to 0.2.2), and still run the downstream cascade idempotently.'

    Workspace = @{
        Preset = 'Linear3'
    }

    History = @(
        # Simulate a prior `release-crate.ps1` run that left b pending at 0.2.1
        # without an intermediate commit.
        @{ Op = 'BumpVersion'; Crate = 'b'; To = '0.2.1' }
    )

    Run = @{
        # Re-invoke for the same pending package without explicit -Change.
        CrateName = 'b'
        BaseRef   = 'HEAD'
        Answers   = @()
    }

    Expect = @{
        # b stays at 0.2.1 (no double-bump). a is the cascade dependent and gets
        # patched (effective base→current bump for b is minor on 0.x.y, which
        # cascades as patch since a doesn't expose b under the default-true
        # allowed_external_types heuristic — but on 0.x.y patch and minor are
        # the same numeric outcome anyway).
        Released = @(
            @{ Crate = 'b'; To = '0.2.1' }
            @{ Crate = 'a'; To = '0.1.1' }
        )
        UnconsumedAnswers = @()
    }
}
