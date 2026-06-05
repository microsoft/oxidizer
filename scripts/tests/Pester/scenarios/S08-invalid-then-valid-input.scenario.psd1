@{
    Name        = 'S08-invalid-then-valid-input'
    Description = 'User provides invalid menu inputs (whole-string check) and empty input before settling on a valid choice. The prompt is repeated each time; no answer is consumed without the menu being shown. Validates strict input validation in Get-PackageReleaseDecision.'

    Workspace = @{ Preset = 'Linear2' }   # downstream -> upstream

    History = @(
        @{ Op = 'ModifySource'; Package = 'downstream' }
        @{ Op = 'ModifySource'; Package = 'upstream' }
        @{ Op = 'AddCommit';    Message = 'upstream edits' }
    )

    Run = @{
        Packages = @('downstream@patch')
        Answers   = @(
            # "12" starts with valid digit but is a multi-character non-option — must be rejected as a whole.
            @{ Match = "Choose option for 'upstream'"; Reply = '12' }
            # Empty input silently re-prompts.
            @{ Match = "Choose option for 'upstream'"; Reply = '' }
            # Finally a valid choice. On 0.x.y the menu offers [1-4] only (option 5
            # is hidden because it would be numerically identical to option 4), so
            # we drive the accept path via '4'.
            @{ Match = "Choose option for 'upstream'"; Reply = '4' }
        )
    }

    Expect = @{
        # upstream accepted as patch → 0.2.0 → 0.2.1. downstream cascade bullet-only at 0.1.1.
        Released = @(
            @{ Package = 'downstream'; To = '0.1.1' }
            @{ Package = 'upstream';   To = '0.2.1' }
        )
        PromptsRaised = @(
            "Choose option for 'upstream'"
            "Choose option for 'upstream'"
            "Choose option for 'upstream'"
        )
        UnconsumedAnswers = @()
    }
}
