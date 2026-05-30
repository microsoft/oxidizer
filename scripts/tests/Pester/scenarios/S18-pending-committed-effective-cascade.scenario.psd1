@{
    Name        = 'S18-pending-committed-effective-cascade'
    Description = 'Mirror of S14 but with the prior bump committed. When the primary target has a committed pending bump at a stronger change than the user re-requests, the cascade must still derive its change type from the EFFECTIVE base→current transition (not the user-requested one) so dependents stay compatible with the on-disk API changes. Verifies that the committed-vs-uncommitted state of the pending bump does not influence the elevation logic.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
    }

    History = @(
        # Prior `release-crate.ps1 -Change NonBreaking` on upstream that has
        # been committed in this branch.
        @{ Op = 'SetVersion'; Package = 'upstream'; To = '1.3.0' }
        @{ Op = 'AddCommit';  Message = 'feat(upstream): release v1.3.0' }
    )

    Run = @{
        # User re-invokes with a WEAKER intent (Patch). The script must NOT
        # downgrade the effective cascade strength: dependents must still
        # receive the change type the on-disk state requires, regardless of
        # whether the pending bump was committed or merely staged.
        PackageName = 'upstream'
        Change    = 'Patch'
        # Default BaseRef = HEAD~1 (baseline commit) so the committed bump
        # registers as pending.
        Answers   = @()
    }

    Expect = @{
        # upstream stays at 1.3.0 (no-op; required-from-base patch = 1.2.4,
        # satisfied by the higher pending 1.3.0). downstream cascades on the
        # EFFECTIVE change (non-breaking on 1.2.3 → 1.3.0): 1.0.0 → 1.1.0,
        # which would NOT have happened if the cascade had used the
        # requested -Change Patch (that would have given 1.0.1 instead).
        Released = @(
            @{ Package = 'upstream';   To = '1.3.0' }
            @{ Package = 'downstream'; To = '1.1.0' }
        )
        UnconsumedAnswers = @()
    }
}
