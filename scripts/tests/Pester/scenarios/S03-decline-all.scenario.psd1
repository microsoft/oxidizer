# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S03-decline-all'
    Description = 'Linear3 with both dependency packages modified: user declines both. Final release is the originally requested package only; both dependency findings stay unreleased.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Package = 'a' }
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'ModifySource'; Package = 'c' }
        @{ Op = 'AddCommit';    Message = 'dependency edits' }
    )

    Run = @{
        Packages = @('a@patch')
        Answers   = @(
            @{ Match = "Choose option for 'b'"; Reply = '2' } # Skip
            @{ Match = "Choose option for 'c'"; Reply = '2' } # Skip
        )
    }

    Expect = @{
        Released = @(
            @{ Package = 'a'; To = '0.1.1' }
        )
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
        )
        UnconsumedAnswers = @()
    }
}
