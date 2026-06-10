# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S20-changed-accept-with-cascade'
    Description = 'Linear3 (a -> b -> c) with all three packages modified. Run in -Mode changed: user accepts b as non-breaking (which cascades a as non-breaking), then ignores c and the cascade-elevated a. Final release set = {b, a}.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Package = 'a' }
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'ModifySource'; Package = 'c' }
        @{ Op = 'AddCommit';    Message = 'edits to all three packages' }
    )

    Run = @{
        Mode    = 'changed'
        Answers = @(
            # Iter 1: queue = [b, c, a]. Accept b as non-breaking (option 4).
            # In 0.x semver this is "patch-style": 0.2.0 -> 0.2.1. We pick
            # non-breaking (not breaking) so the cascade onto 'a' lands as
            # non-breaking too, which keeps 'a' eligible for the elevation-
            # review prompt below. (A breaking cascade would mark 'a' as
            # Source='cascade' with EffectiveChangeType='breaking' and the
            # surfacing predicate would skip it.)
            @{ Match = "Choose option for 'b'"; Reply = '4' } # Non-breaking
            # Iter 2: after accepting b@nonbreaking, Resolve-ReleaseSet pulls
            # 'a' in as a cascade non-breaking. Findings now surface c (still
            # modified+unreleased) and a (Source='cascade' + non-breaking, so
            # eligible for elevation review). User ignores c.
            @{ Match = "Choose option for 'c'"; Reply = '2' } # No material changes
            # User leaves a at its cascade-applied non-breaking level (ignore
            # = "keep cascade-applied level"; recorded into $reviewedCascadeAsIs
            # so it does not re-surface on the next iteration).
            @{ Match = "Choose option for 'a'"; Reply = '2' } # No material changes
        )
    }

    Expect = @{
        # 0.x semver: b at 0.2.0 non-breaking -> 0.2.1; a at 0.1.0 cascade
        # non-breaking -> 0.1.1. c stays unreleased.
        Released = @(
            @{ Package = 'b'; To = '0.2.1' }
            @{ Package = 'a'; To = '0.1.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
            "Choose option for 'a'"
        )
        UnconsumedAnswers = @()
    }
}
