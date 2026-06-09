# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S10-stable-version-patch-distinct'
    Description = 'Stable (>=1.x.y) workspace: the menu offers all 5 options because non-breaking (option 4) and patch (option 5) produce distinct numeric outcomes. User picks option 5 on dependency; verifies dependency ends at 1.2.4 (a patch change), proving the patch path is reachable end-to-end and is NOT a synonym for option 4 (which would yield 1.3.0). Counter-balances the 0.x-only synthetic-workspace presets, which collapse 4 and 5 into the same numeric increment.'

    # Inline spec: every built-in preset uses 0.x versions, so we hand-roll a
    # stable two-package topology here to exercise the >=1.x.y branch of
    # Get-NextVersion and the [1-5] menu range.
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
        Answers   = @(
            # On a stable >=1.x.y package the menu offers [1-5]; '5' selects the
            # patch path (Action='patch'), which is numerically distinct from
            # option 4 ('non-breaking' would produce 1.3.0).
            @{ Match = "Choose option for 'dependency'"; Reply = '5' } # Patch
        )
    }

    Expect = @{
        # dependent: 1.0.0 -> 1.0.1 (the explicit -Change Patch on the user-named release).
        # dependency  : 1.2.3 -> 1.2.4 (patch chosen via option 5 — distinct from option 4 which
        # would have given 1.3.0). The [1-5] suffix in PromptsRaised pins the menu range too.
        Released = @(
            @{ Package = 'dependent'; To = '1.0.1' }
            @{ Package = 'dependency';   To = '1.2.4' }
        )
        PromptsRaised = @(
            "Choose option for 'dependency' [1-5]"
        )
        UnconsumedAnswers = @()
    }
}
