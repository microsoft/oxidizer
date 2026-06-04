@{
    Name        = 'S22-all-no-modifications-ignore-all'
    Description = 'Linear3 (a -> b -> c) with NO modifications. Run in -Mode all: the planner surfaces every publishable package for review (despite the empty change set) and the user ignores each in turn. Expect no releases and a prompt per package. Validates that -All bypasses the change-detection filter that -Changed enforces.'

    Workspace = @{ Preset = 'Linear3' }   # a -> b -> c

    History = @()  # no modifications

    Run = @{
        # Mode='all' invokes Invoke-ReleasePackagesMain -Mode 'all'. The
        # snapshot is synthesised in the entry point so every published
        # package is surfaced as a BFS root. Initial walk order matches
        # S19/S20 because the BFS still records chains via 'a' first
        # (sorted root order: a, b, c → a's BFS visits b then c, c's BFS
        # is empty, and Phase-B sweep adds a as a stub finding).
        Mode    = 'all'
        Answers = @(
            @{ Match = "Choose option for 'b'"; Reply = '2' }
            @{ Match = "Choose option for 'c'"; Reply = '2' }
            @{ Match = "Choose option for 'a'"; Reply = '2' }
        )
    }

    Expect = @{
        # No releases — every package was ignored. The point of this scenario
        # is to prove -All surfaces unchanged packages at all (something
        # -Changed would skip), not to release anything.
        PromptsRaised = @(
            "Choose option for 'b'"
            "Choose option for 'c'"
            "Choose option for 'a'"
        )
        UnconsumedAnswers = @()
    }
}
