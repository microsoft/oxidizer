# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S11-stable-version-minor-distinct'
    Description = 'Companion to S10: same stable workspace, but the user picks option 4 (non-breaking) on dependency. Verifies dependency ends at 1.3.0 (a non-breaking change), confirming that on >=1.x.y packages options 4 and 5 resolve to genuinely different on-disk versions.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'dependent'; Version = '1.0.0'; Deps = @(@{ Name = 'dependency' }) }
                @{ Name = 'dependency';   Version = '1.2.3' }
            )
        }
    }

    History = @(
        @{ Op = 'ModifySource'; Package = 'dependent' }
        @{ Op = 'ModifySource'; Package = 'dependency' }
        @{ Op = 'AddCommit';    Message = 'dependency edits' }
    )

    Run = @{
        Packages = @('dependent@patch')
        # dependent re-exports dependency's public types; when dependency lands a
        # non-breaking change, dependent's own API gains those additions, so
        # cargo-semver-checks classifies dependent as non-breaking (→ 1.1.0).
        SemverVerdicts = @{ dependent = 'non-breaking' }
        Answers   = @(
            # On a stable >=1.x.y package the menu offers [1-5]; '4' selects the
            # minor (non-breaking) path, distinct from the patch path of option 5.
            @{ Match = "Choose option for 'dependency'"; Reply = '4' } # Non-breaking
        )
    }

    Expect = @{
        # dependent: 1.0.0 -> 1.1.0. The user requested -Change Patch, but the
        # post-release scan accepts dependency as a *non-breaking* change
        # (1.2.3 -> 1.3.0). The cascade then escalates dependent to a
        # non-breaking change too — on stable >=1.x.y, a non-breaking change
        # in an dependency propagates as non-breaking in dependents (see
        # Test-IsBreakingChange + the exposing-cascade logic in
        # Invoke-ReleaseFlow).
        # dependency: 1.2.3 -> 1.3.0 (non-breaking; option 5 would have given 1.2.4).
        Released = @(
            @{ Package = 'dependent'; To = '1.1.0' }
            @{ Package = 'dependency';   To = '1.3.0' }
        )
        PromptsRaised = @(
            "Choose option for 'dependency' [1-5]"
        )
        UnconsumedAnswers = @()
    }
}
