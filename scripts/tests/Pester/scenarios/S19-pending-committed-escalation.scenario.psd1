@{
    Name        = 'S19-pending-committed-escalation'
    Description = 'A committed pending bump can be escalated to a stronger change type by re-invoking `release-crate.ps1` with the higher change. This is the critical "I picked the wrong change type; let me elevate it" workflow when the prior version increment has already been committed in the branch. The script must NOT add a second increment on top of the existing pending one — it must re-stamp Cargo.toml in place so the base→current transition reflects the new, stronger change type.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'upstream' }) }
                @{ Name = 'upstream';   Version = '1.2.3' }
            )
        }
    }

    History = @(
        # Prior `release-crate.ps1 -Change Patch` on upstream that has been
        # committed in this branch (base 1.2.3 → pending 1.2.4).
        @{ Op = 'SetVersion'; Package = 'upstream'; To = '1.2.4' }
        @{ Op = 'AddCommit';  Message = 'feat(upstream): release v1.2.4' }
    )

    Run = @{
        # User realises the change is actually non-breaking and re-invokes
        # with the stronger intent. Required-from-base non-breaking on 1.2.3
        # is 1.3.0, which is higher than the pending 1.2.4 — so the script
        # escalates by re-stamping Cargo.toml in place (not by incrementing
        # the already-incremented pending version a second time).
        PackageName = 'upstream'
        Change    = 'NonBreaking'
        Answers   = @()
    }

    Expect = @{
        # upstream is escalated 1.2.4 → 1.3.0 (not 1.2.5, which would be the
        # wrong "increment on top of pending" result). The OldVersion in the
        # release record reflects the base (1.2.3), not the intermediate
        # 1.2.4 — the intermediate is an implementation detail of the
        # in-place re-stamping.
        Released = @(
            @{ Package = 'upstream';   To = '1.3.0' }
            @{ Package = 'downstream'; To = '1.1.0' }
        )
        UnconsumedAnswers = @()
    }
}
