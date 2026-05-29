@{
    Name        = 'S06-cascade-mid-foreach-skip'
    Description = 'Multi-release-set topology that naturally produces iteration order [b, a] where accepting b cascade-bumps a into the release set. a is then encountered later in the same foreach and its prompt is SKIPPED via the currentReleaseSet membership check.

Spec: zeta and alpha are both pre-bumped (release set = {alpha, zeta}). alpha → b; zeta → a; a → b. With Sort-Object on the release-set foreach, alpha enumerates first → b is inserted into findings before a. Accepting b cascade-bumps both alpha (pre-bumped, sufficient) and a (newly bumped). When the foreach next reaches a, the skip path fires.'

    Workspace = @{
        Spec = @{
            Crates = @(
                @{ Name = 'zeta';  Version = '0.1.0'; Deps = @(@{ Name = 'a' }) }
                @{ Name = 'alpha'; Version = '0.2.0'; Deps = @(@{ Name = 'b' }) }
                @{ Name = 'a';     Version = '0.3.0'; Deps = @(@{ Name = 'b' }) }
                @{ Name = 'b';     Version = '0.4.0' }
            )
        }
    }

    History = @(
        # Pre-bump zeta and alpha as part of the simulated PR.
        @{ Op = 'BumpVersion'; Crate = 'zeta';  To = '0.1.1' }
        @{ Op = 'BumpVersion'; Crate = 'alpha'; To = '0.2.1' }
        @{ Op = 'AddCommit';   Message = 'pre-bump release set' }
        # Modify upstreams (publishable changes).
        @{ Op = 'ModifySource'; Crate = 'a' }
        @{ Op = 'ModifySource'; Crate = 'b' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        # User invokes for zeta (any pre-bumped crate works); script will detect the
        # pre-bump and proceed with the cascade + post-release scan.
        CrateName = 'zeta'
        BaseRef   = 'HEAD~2'
        Answers   = @(
            @{ Match = "Choose option for 'b'"; Reply = '5' }
            # No prompt expected for 'a': it was cascade-bumped by accepting 'b' and
            # is filtered out of the next iteration's queue by the BFS.
        )
    }

    Expect = @{
        # zeta started at 0.1.0, pre-bumped to 0.1.1, then re-bumped by the explicit
        # Invoke-ReleaseMain run (default 'minor' on 0.1.1 → 0.1.2 under the 0.x convention).
        # b accepted as patch → 0.4.0 → 0.4.1; b's cascade upgrades a (0.3.0 → 0.3.1)
        # and bullet-onlys alpha (pre-bumped to 0.2.1 already satisfies the patch requirement).
        Released = @(
            @{ Crate = 'zeta';  To = '0.1.2' }
            @{ Crate = 'alpha'; To = '0.2.1' }
            @{ Crate = 'b';     To = '0.4.1' }
            @{ Crate = 'a';     To = '0.3.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
        )
        UnconsumedAnswers = @()
    }
}

