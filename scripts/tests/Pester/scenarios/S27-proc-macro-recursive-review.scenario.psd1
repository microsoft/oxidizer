# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S27-proc-macro-recursive-review'
    Description = 'Manual review advances one published dependency edge at a time: a breaking proc macro surfaces its facade, and a breaking facade then surfaces its direct consumer.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'app'; Version = '1.0.0'; Deps = @(@{ Name = 'facade' }) }
                @{ Name = 'facade'; Version = '1.0.0'; Deps = @(@{ Name = 'macros' }) }
                @{ Name = 'macros'; Version = '1.0.0'; ProcMacro = $true }
            )
        }
    }

    History = @()

    Run = @{
        Packages = @('macros@patch')
        Answers = @(
            @{ Match = "Choose option for 'macros'"; Reply = '3' }
            @{ Match = "Choose option for 'facade'"; Reply = '3' }
            @{ Match = "Choose option for 'app'"; Reply = '2' }
        )
    }

    Expect = @{
        Released = @(
            @{ Package = 'macros'; To = '2.0.0' }
            @{ Package = 'facade'; To = '2.0.0' }
            @{ Package = 'app'; To = '1.0.1' }
        )
        PromptsRaised = @(
            "Choose option for 'macros'"
            "Choose option for 'facade'"
            "Choose option for 'app'"
        )
        UnconsumedAnswers = @()
    }
}
