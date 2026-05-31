@{
    Name        = 'S18-graduation-rejected-on-stable'
    Description = 'Bundled-input version-graduation safety contract: user uses the ''1.0.0'' graduation token on a package that is already at >=1.0.0. Resolve-ReleaseSet throws because graduation can only happen once and the package is already past it. The user must use a change-type keyword or an explicit pin > current.'

    Workspace = @{
        Spec = @{
            Packages = @(
                @{ Name = 'already-stable'; Version = '1.4.2' }
            )
        }
    }

    History = @()

    Run = @{
        Packages = @('already-stable@1.0.0')
        Answers  = @()
    }

    Expect = @{
        Throws        = $true
        ThrowsMatches = 'already-stable'
        Released      = @()
        PromptsRaised = @()
        UnconsumedAnswers = @()
    }
}
