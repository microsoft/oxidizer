# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

@{
    Name        = 'S14-pin-with-cascade-conflict'
    Description = 'Bundled-input safety contract: user pins a target version that the cascade analysis determines is numerically too low. Resolve-ReleaseSet throws a clear error directing the user to revise the pin or use a change-type keyword. This is the design that gives explicit pins a strong guarantee — when used, the pin is honoured verbatim; when impossible, the script refuses to proceed silently.'

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
        # target releases as breaking → 2.0.0. Cascade requires dependent at >=2.0.0.
        # User pinned dependent at 1.0.1 which is below the cascade requirement →
        # Resolve-ReleaseSet must throw.
        Packages = @('target@breaking', 'dependent@1.0.1')
        Answers  = @()
    }

    Expect = @{
        # No releases produced; the run terminates with an exception before
        # any on-disk Cargo.toml is rewritten.
        Throws            = $true
        ThrowsMatches     = "Cannot release 'dependent' as v1.0.1"
        Released          = @()
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
