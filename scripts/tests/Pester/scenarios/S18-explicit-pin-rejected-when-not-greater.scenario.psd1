@{
    Name        = 'S18-explicit-pin-rejected-when-not-greater'
    Description = 'Bundled-input explicit-pin safety contract: user supplies an explicit ''1.0.0'' semver pin on a package already at >= 1.0.0. The planner throws because explicit version pins must be strictly greater than the current on-disk version. (1.0.0 has no special meaning — this is the same error any not-strictly-greater pin would raise.)'

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
        ThrowsMatches = 'already at v1.4.2'
        Released      = @()
        PromptsRaised = @()
        UnconsumedAnswers = @()
    }
}
