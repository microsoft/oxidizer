@{
    Name        = 'S19-changed-ignore-everything'
    Description = 'Linear3 (a -> b -> c) with all three packages modified. Run in -Mode changed and ignore every prompt; expect no releases and no errors. Validates the early-exit path when the user declines every surfaced finding.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @(
        @{ Op = 'ModifySource'; Package = 'a' }
        @{ Op = 'ModifySource'; Package = 'b' }
        @{ Op = 'ModifySource'; Package = 'c' }
        @{ Op = 'AddCommit';    Message = 'edits to all three packages' }
    )

    Run = @{
        # Mode='changed' invokes Invoke-ReleasePackagesMain -Mode 'changed'
        # (no -Packages list). The review loop seeds BFS roots from every
        # changed package, so the user is walked through b, c, a in that
        # order — b and c come first as BFS-recorded dependencies of a;
        # a comes last as a Phase-B stub (no in-release-set dependents).
        Mode    = 'changed'
        Answers = @(
            @{ Match = "Choose option for 'b'"; Reply = '2' }
            @{ Match = "Choose option for 'c'"; Reply = '2' }
            @{ Match = "Choose option for 'a'"; Reply = '2' }
        )
    }

    Expect = @{
        # No releases — every package was ignored.
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
            "Choose option for 'a'"
        )
        UnconsumedAnswers = @()
    }
}
