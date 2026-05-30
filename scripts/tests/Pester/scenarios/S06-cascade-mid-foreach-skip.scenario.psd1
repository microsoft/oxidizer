@{
    Name        = 'S06-cascade-mid-foreach-skip'
    Description = 'Multi-release-set topology where accepting b cascade-bumps a into the release set. Per Invariant B, because a is ALSO modified (pre-existing changes) AND the cascade-applied bump on a is non-breaking, a must be re-surfaced for elevation review. User ignores → a stays at the cascade-applied 0.3.1.

Spec: zeta and alpha are both pre-bumped (release set = {alpha, zeta}). alpha → b; zeta → a; a → b. With Sort-Object on the release-set foreach, alpha enumerates first → b is inserted into findings before a. Accepting b cascade-bumps both alpha (pre-bumped, sufficient) and a (newly bumped). a being modified + cascade-bumped non-breaking triggers Invariant B prompt; user ignores.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'zeta';  Version = '0.1.0'; Deps = @(@{ Name = 'a' }) }
                @{ Name = 'alpha'; Version = '0.2.0'; Deps = @(@{ Name = 'b' }) }
                @{ Name = 'a';     Version = '0.3.0'; Deps = @(@{ Name = 'b' }) }
                @{ Name = 'b';     Version = '0.4.0' }
            )
        }
    }

    History = @(
        # Pre-bump zeta and alpha as part of the simulated PR.
        @{ Op = 'BumpVersion'; Package = 'zeta';  To = '0.1.1' }
        @{ Op = 'BumpVersion'; Package = 'alpha'; To = '0.2.1' }
        @{ Op = 'AddCommit';   Message = 'pre-bump release set' }
        # Modify upstreams (publishable changes).
        @{ Op = 'ModifySource'; Package = 'a' }
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        # User invokes for zeta (any pre-bumped package works); script will detect the
        # pre-bump and proceed with the cascade + post-release scan.
        PackageName = 'zeta'
        BaseRef   = 'HEAD~2'
        Answers   = @(
            # On 0.x.y the menu hides option 5 (patch), so accept via option 4
            # (non-breaking) — same 0.x.(y+1) outcome under Cargo semver.
            @{ Match = "Choose option for 'b'"; Reply = '4' }
            # Invariant B: 'a' was cascade-bumped into the release set with a
            # non-breaking bump AND also has pre-existing modifications, so the
            # next BFS iteration re-surfaces 'a' for elevation review. User
            # answers '2' (ignore — keep the cascade-applied bump).
            @{ Match = "Choose option for 'a'"; Reply = '2' }
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
            @{ Package = 'zeta';  To = '0.1.1' }
            @{ Package = 'alpha'; To = '0.2.1' }
            @{ Package = 'b';     To = '0.4.1' }
            @{ Package = 'a';     To = '0.3.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'a'"
        )
        UnconsumedAnswers = @()
    }
}

