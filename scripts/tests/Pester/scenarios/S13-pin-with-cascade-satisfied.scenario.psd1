# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S13-pin-with-cascade-satisfied'
    Description = 'Bundled-input feature: user explicitly pins a target version for one package, and a cascade from another user-source package strengthens the change-type tag but the pin still numerically satisfies the cascade requirement. Validates that Resolve-ReleaseSet keeps the pinned version verbatim (does not bump it to the cascade-required minimum) while still updating EffectiveChangeType so any dependent cascade decisions are correct.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'dependent'; Version = '1.0.0'; Deps = @(@{ Name = 'target' }) }
                @{ Name = 'target';    Version = '1.0.0' }
            )
        }
    }

    History = @()

    Run = @{
        # target releases as breaking → 2.0.0. Cascade requires dependent at >=2.0.0
        # because dependent exposes target. User pins dependent at 5.0.0 which
        # satisfies the cascade requirement, so the pin wins.
        Packages = @('target@breaking', 'dependent@5.0.0')
        Answers  = @()
    }

    Expect = @{
        Released = @(
            @{ Package = 'target';    To = '2.0.0' }
            @{ Package = 'dependent'; To = '5.0.0' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
