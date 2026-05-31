@{
    Name        = 'S17-graduation-token'
    Description = 'Bundled-input version-graduation contract: user uses the explicit ''1.0.0'' graduation token on a 0.x.y package. Resolve-ReleaseSet treats this as a pinned target version and accepts it because the package is currently in 0.x.y range. This is the standard "drop the leading zero" lifecycle event done exactly once per package.'

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
