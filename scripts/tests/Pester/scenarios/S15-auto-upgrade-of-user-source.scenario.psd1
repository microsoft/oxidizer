@{
    Name        = 'S15-auto-upgrade-of-user-source'
    Description = 'Bundled-input cascade behaviour: user requests a weak change type for a package, but a cascade from another user-source package mandates a stronger change type. Resolve-ReleaseSet auto-upgrades the user-source entry and Show-ReleasePlan flags the upgrade with the ''auto-upgraded by cascade'' tag so the user has visibility into what happened. No prompt is raised — the upgrade is silent (no user judgement needed; cascade rules are deterministic).'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'dependent'; Version = '1.0.0'; Deps = @(@{ Name = 'target' }) }
                @{ Name = 'target';    Version = '1.0.0' }
            )
        }
    }

    History = @()

    Run = @{
        # User wants dependent at patch (1.0.0 → 1.0.1), but target releases
        # as breaking (1.0.0 → 2.0.0) and dependent exposes target → cascade
        # required-level is breaking. Resolve-ReleaseSet auto-upgrades
        # dependent's EffectiveChangeType to breaking and EffectiveTargetVersion
        # to 2.0.0.
        Packages = @('target@breaking', 'dependent@patch')
        Answers  = @()
    }

    Expect = @{
        Released = @(
            @{ Package = 'target';    To = '2.0.0' }
            @{ Package = 'dependent'; To = '2.0.0' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
