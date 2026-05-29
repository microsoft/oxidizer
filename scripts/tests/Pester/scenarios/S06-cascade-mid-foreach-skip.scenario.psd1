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
            # On 0.x.y the menu hides option 5 (patch), so accept via option 4
            # (non-breaking) — same 0.x.(y+1) outcome under Cargo semver.
            @{ Match = "Choose option for 'b'"; Reply = '4' }
            # No prompt expected for 'a': it was cascade-bumped by accepting 'b' and
            # is filtered out of the next iteration's queue by the BFS.
        )
    }

    Expect = @{
        # With cross-invocation pending-release detection, zeta is recognised as
        # already pending (base 0.1.0 → current 0.1.1) and the primary bump is
        # skipped instead of double-bumping to 0.1.2 as the legacy flow did.
        # The cascade still runs with the EFFECTIVE base→current bump (minor
        # on 0.x.y per Cargo semver), so a (which exposes target) takes patch
        # from b's downstream cascade and alpha (pre-bumped) stays bullet-only.
        Released = @(
            @{ Crate = 'zeta';  To = '0.1.1' }
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

