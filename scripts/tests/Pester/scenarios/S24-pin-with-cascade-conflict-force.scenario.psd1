# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S24-pin-with-cascade-conflict-force'
    Description = '-Force overrides the pin-vs-cascade rejection: same workspace and pin as S14, but the user passes -Force so the explicit pin (dependent@1.0.1) is honoured verbatim even though cascade requires >=2.0.0. The package emits a warning (visible to a maintainer running interactively), Resolve-ReleaseSet tags the entry PinHonoredAgainstCascade, and Invoke-ResolvedRelease writes the pinned version on disk. The mirror-image scenario S14 covers the rejection path; this one covers the override path. End-to-end coverage proves the -Force switch is plumbed all the way from Invoke-ReleasePackagesMain through Invoke-PlanReview into Resolve-ReleaseSet (unit coverage of the resolver alone cannot demonstrate the parameter wiring).'

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
        Packages = @('target@breaking', 'dependent@1.0.1')
        SemverVerdicts = @{ dependent = 'breaking' }
        Force    = $true
        Answers  = @()
    }

    Expect = @{
        # No exception this time: -Force converts the rejection into a warning.
        Throws = $false
        Released = @(
            @{ Package = 'target';    To = '2.0.0' }
            # Cascade required >=2.0.0, but the user's pin (1.0.1) is honoured
            # verbatim under -Force. The on-disk version is the pin.
            @{ Package = 'dependent'; To = '1.0.1' }
        )
        # The workspace has no modifications, so no per-package elevation
        # prompts fire; -Force only converts the resolver's throw into a
        # warning (Write-Warning, not Read-Host).
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
