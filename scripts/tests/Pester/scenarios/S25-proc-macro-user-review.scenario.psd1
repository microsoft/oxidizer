# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S25-proc-macro-user-review'
    Description = 'A breaking user-selected proc macro triggers manual review of its direct published consumer. Keeping that consumer at patch stops review propagation, while the next-level dependent retains normal cascade classification.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'downstream'; Version = '1.0.0'; Deps = @(@{ Name = 'consumer' }) }
                @{ Name = 'consumer'; Version = '1.0.0'; Deps = @(@{ Name = 'macros' }) }
                @{ Name = 'macros'; Version = '1.0.0'; ProcMacro = $true }
            )
        }
    }

    History = @()

    Run = @{
        Packages = @('macros@patch')
        Answers = @(
            @{ Match = "Choose option for 'macros'"; Reply = '1' }
            @{ Match = "Choose option for 'macros'"; Reply = '3' }
            @{ Match = "Choose option for 'consumer'"; Reply = '2' }
        )
    }

    Expect = @{
        Released = @(
            @{ Package = 'macros'; To = '2.0.0' }
            @{ Package = 'consumer'; To = '1.0.1' }
            @{ Package = 'downstream'; To = '1.0.1' }
        )
        PromptsRaised = @(
            "Choose option for 'macros'"
            "Choose option for 'macros'"
            "Choose option for 'consumer'"
        )
        UnconsumedAnswers = @()
    }
}
