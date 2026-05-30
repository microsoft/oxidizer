@{
    Name        = 'S14-pending-effective-cascade-change'
    Description = 'When the primary target is already pending at a stronger change than the user requests on re-invocation, the cascade must derive its change type from the EFFECTIVE base→current transition (not the user-requested one) so dependents stay compatible with the on-disk API changes. Here: upstream is pending as a non-breaking change (1.2.3 → 1.3.0); user re-invokes with -Change Patch; downstream must still receive the equivalent of a minor cascade (1.0.0 → 1.1.0), NOT a patch one (1.0.1).'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
    }

    History = @(
        # Prior `release-crate.ps1 -Change NonBreaking` on upstream left it
        # pending at 1.3.0 (a non-breaking change).
        @{ Op = 'SetVersion'; Package = 'upstream'; To = '1.3.0' }
    )

    Run = @{
        # User re-invokes with a WEAKER intent (Patch). The script must NOT
        # downgrade the effective cascade strength: dependents must still
        # receive the change type the on-disk state requires.
        PackageName = 'upstream'
        Change    = 'Patch'
        BaseRef   = 'HEAD'
        Answers   = @()
    }

    Expect = @{
        # upstream stays at 1.3.0 (no-op; required-from-base patch = 1.2.4,
        # which is satisfied by the higher pending 1.3.0). downstream cascades
        # as the EFFECTIVE change (minor on 1.2.3 → 1.3.0), so 1.0.0 → 1.1.0,
        # which would NOT have happened if the cascade had used the requested
        # -Change Patch (that would have given 1.0.1 instead).
        Released = @(
            @{ Package = 'upstream';   To = '1.3.0' }
            @{ Package = 'downstream'; To = '1.1.0' }
        )
        UnconsumedAnswers = @()
    }
}
