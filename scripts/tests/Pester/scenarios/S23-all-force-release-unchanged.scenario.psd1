# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S23-all-force-release-unchanged'
    Description = 'Linear3 (a -> b -> c) with NO modifications. Run in -Mode all: user ignores b, then accepts c as breaking. The cascade-toward-dependents walk pulls b and a in as cascade-breaking (mirroring c''s change type), which silences the elevation prompt for both (Invariant B: cascade entries that already match the strongest change type are not re-surfaced). Net result: c, b, and a all released at breaking version bumps. Validates that -All can release packages with no on-disk changes, and that cascade-breaking suppresses follow-up prompts.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @()  # no modifications — -All surfaces packages regardless

    Run = @{
        Mode    = 'all'
        # a and b re-export c's public types, so when c lands a breaking change
        # their own APIs break too — cargo-semver-checks classifies both as
        # breaking. This is what makes the cascade mirror c's breaking onto
        # them (and, being at the breaking ceiling, suppresses their prompts).
        SemverVerdicts = @{ a = 'breaking'; b = 'breaking' }
        Answers = @(
            # Initial queue (no tokens, all roots): b, c, a (see S22 comment
            # for the BFS-root expansion that produces this order).
            #
            # We have no decision for b yet; ignore so c is reached as the
            # breaking trigger. 'b' is recorded in $declined for this run
            # (b is not InReleaseSet at this point: no tokens, no cascade).
            @{ Match = "Choose option for 'b'"; Reply = '2' } # No material changes
            # Accept c as breaking (option 3). c: 0.3.0 -> 0.4.0. The
            # cascade-toward-dependents walk mirrors c's change type onto b
            # and onto a (transitively), so both arrive in the release set
            # as cascade-breaking. The surfacing predicate skips cascade
            # entries already at the "breaking" ceiling (Invariant B), so
            # neither b nor a is re-prompted, AND a is dropped from the
            # initial queue (it was a Phase-B stub before c's acceptance;
            # the next iteration sees it as cascade-breaking and filters it
            # out).
            @{ Match = "Choose option for 'c'"; Reply = '3' } # Breaking
        )
    }

    Expect = @{
        # 0.x cargo rules: breaking 0.x.y -> 0.(x+1).0.
        # c: 0.3.0 -> 0.4.0 (user-accepted breaking).
        # b: 0.2.0 -> 0.3.0 (cascade breaking from c).
        # a: 0.1.0 -> 0.2.0 (cascade breaking from b).
        Released = @(
            @{ Package = 'c'; To = '0.4.0' }
            @{ Package = 'b'; To = '0.3.0' }
            @{ Package = 'a'; To = '0.2.0' }
        )
        # Only b and c are prompted. a never reaches the prompt: when c is
        # accepted in iter 2, the re-resolve marks a as cascade-breaking,
        # which the surfacing predicate filters out before the prompt would
        # fire. This is the same elevation-suppression logic that protects
        # users from being asked to re-confirm a release already at the
        # ceiling change type.
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
        )
        UnconsumedAnswers = @()
    }
}
