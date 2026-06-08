# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S12-multi-package-single-invocation'
    Description = 'Bundled-input core capability: one invocation releases two packages with independent change types. No dependency between the two packages, so no cascade interaction. Validates that Parse-ReleaseTokens and Resolve-ReleaseSet handle multi-token input and that Invoke-ResolvedRelease processes both in topo order without interfering.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'alpha'; Version = '1.2.3' }
                @{ Name = 'beta';  Version = '0.4.5' }
            )
        }
    }

    History = @()

    Run = @{
        # Two independent packages, two distinct change types in one invocation.
        Packages = @('alpha@nonbreaking', 'beta@breaking')
        Answers  = @()
    }

    Expect = @{
        # alpha: 1.2.3 -> 1.3.0 (non-breaking on stable).
        # beta:  0.4.5 -> 0.5.0 (breaking on 0.x; 0.x breaking is minor numerically).
        Released = @(
            @{ Package = 'alpha'; To = '1.3.0' }
            @{ Package = 'beta';  To = '0.5.0' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
