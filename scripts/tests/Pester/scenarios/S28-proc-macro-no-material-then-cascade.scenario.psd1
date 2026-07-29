# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S28-proc-macro-no-material-then-cascade'
    Description = 'Choosing no material changes completes proc-macro review. If a later decision pulls that proc macro into the release plan, it gets the patch floor without a second prompt.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'seed'; Version = '1.0.0'; Deps = @(@{ Name = 'macros' }) }
                @{ Name = 'macros'; Version = '1.0.0'; ProcMacro = $true; Deps = @(@{ Name = 'implementation' }) }
                @{ Name = 'implementation'; Version = '1.0.0' }
            )
        }
    }

    History = @(
        @{ Op = 'ModifySource'; Package = 'seed' }
        @{ Op = 'ModifySource'; Package = 'macros' }
        @{ Op = 'ModifySource'; Package = 'implementation' }
        @{ Op = 'AddCommit'; Message = 'package edits' }
    )

    Run = @{
        Packages = @('seed@patch')
        Answers = @(
            @{ Match = "Choose option for 'macros'"; Reply = '2' }
            @{ Match = "Choose option for 'implementation'"; Reply = '5' }
        )
    }

    Expect = @{
        Released = @(
            @{ Package = 'seed'; To = '1.0.1' }
            @{ Package = 'macros'; To = '1.0.1' }
            @{ Package = 'implementation'; To = '1.0.1' }
        )
        PromptsRaised = @(
            "Choose option for 'macros'"
            "Choose option for 'implementation'"
        )
        UnconsumedAnswers = @()
    }
}
