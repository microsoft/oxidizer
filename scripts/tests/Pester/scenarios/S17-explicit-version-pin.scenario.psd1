@{
    Name        = 'S17-explicit-version-pin'
    Description = 'Bundled-input explicit-pin contract: user supplies an explicit ''1.0.0'' semver pin on a 0.x.y package. The planner accepts the pin because the package is currently below 1.0.0 (pin must be strictly greater than the current version). 1.0.0 has no special handling — it is treated like any other explicit pin.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'graduating'; Version = '0.7.2' }
            )
        }
    }

    History = @()

    Run = @{
        Packages = @('graduating@1.0.0')
        Answers  = @()
    }

    Expect = @{
        Released = @(
            @{ Package = 'graduating'; To = '1.0.0' }
        )
        PromptsRaised     = @()
        UnconsumedAnswers = @()
    }
}
