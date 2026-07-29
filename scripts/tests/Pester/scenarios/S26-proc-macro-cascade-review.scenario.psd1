# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S26-proc-macro-cascade-review'
    Description = 'A proc-macro-only dependent pulled into the release set by an implementation-crate release is manually reviewed even when its own package folder is unchanged. Selecting a non-breaking release replaces the mechanical patch floor and does not trigger downstream manual review.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'consumer'; Version = '1.0.0'; Deps = @(@{ Name = 'macros' }) }
                @{ Name = 'macros'; Version = '1.0.0'; ProcMacro = $true; Deps = @(@{ Name = 'implementation' }) }
                @{ Name = 'implementation'; Version = '1.0.0' }
            )
        }
    }

    History = @()

    Run = @{
        Packages = @('implementation@patch')
        Answers = @(
            @{ Match = "Choose option for 'macros'"; Reply = '4' }
        )
    }

    Expect = @{
        Released = @(
            @{ Package = 'implementation'; To = '1.0.1' }
            @{ Package = 'macros'; To = '1.1.0' }
            @{ Package = 'consumer'; To = '1.0.1' }
        )
        PromptsRaised = @(
            "Choose option for 'macros'"
        )
        UnconsumedAnswers = @()
    }
}
