# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

BeforeAll {
    . (Join-Path $PSScriptRoot '..\..\_common\TestHelpers.ps1')

    $script:RepoRoot = Get-OxiRepoRoot
}

Describe 'Workspace example target names' {
    # Cargo writes every example in the workspace into one shared
    # target/<profile>/examples/ directory, so two packages declaring the same
    # example name resolve to the same output file. Cargo only *warns* about
    # the exact-duplicate case ("output filename collision ... this may become
    # a hard error in the future", rust-lang/cargo#6313) and then builds both
    # targets concurrently into that one path. On Windows the resulting
    # link.exe race is fatal -- PR #676 renamed a duplicate `tower_service`
    # example after it produced LNK1104 on an unrelated pull request. On Linux
    # and macOS one binary silently overwrites the other, which is worse: the
    # example that runs is not the one that was selected and nothing fails.
    #
    # This lives in the repository checks rather than in an Anvil recipe for
    # two reasons. The Anvil recipes are generated (`DO NOT EDIT DIRECTLY`), so
    # a check added there is reverted on the next regeneration; and the Anvil
    # groups are impact-scoped, while a duplicate example name is a property of
    # the whole workspace. A scoped check would pass on exactly the pull
    # requests that did not touch either colliding crate.
    It 'declares every example target name exactly once across the workspace' {
        Push-Location $script:RepoRoot
        try {
            $metadataJson = & cargo metadata --no-deps --format-version 1
            $LASTEXITCODE | Should -Be 0 -Because 'cargo metadata must succeed'
        } finally {
            Pop-Location
        }

        $metadata = $metadataJson | ConvertFrom-Json

        $declarations = foreach ($package in $metadata.packages) {
            foreach ($target in $package.targets) {
                if ($target.kind -contains 'example') {
                    [pscustomobject]@{
                        Package = $package.name
                        Example = $target.name
                    }
                }
            }
        }

        $declarations | Should -Not -BeNullOrEmpty -Because 'the workspace has example targets to check'

        # Grouped case-insensitively on purpose. Windows and the default
        # case-insensitive macOS volumes resolve `basic` and `Basic` to one
        # path, but Cargo compares PathBufs -- which are case-sensitive in Rust
        # -- so it does not even emit its usual warning for that variant. The
        # case-only collision is therefore the more dangerous of the two:
        # silent on both sides. Cargo target names are ASCII, so folding with
        # ToLowerInvariant is exact here rather than an approximation.
        $collisions = @(
            $declarations |
                Group-Object -Property { $_.Example.ToLowerInvariant() } |
                Where-Object Count -GT 1
        )

        if ($collisions.Count -gt 0) {
            $detail = foreach ($collision in $collisions) {
                # Spell out each package's own casing only when the spellings
                # actually differ, so the common exact-duplicate message stays
                # terse and a case-only collision is not mistaken for a
                # reporting bug.
                $spellings = @($collision.Group.Example | Sort-Object -Unique)
                $owners = if ($spellings.Count -eq 1) {
                    (@($collision.Group.Package | Sort-Object) -join ', ')
                } else {
                    (@($collision.Group | Sort-Object Package | ForEach-Object { "$($_.Package) ('$($_.Example)')" }) -join ', ')
                }
                "  - '$($collision.Name)' is declared by: $owners"
            }

            $message = @(
                "Example target names must be unique across the workspace, but $($collisions.Count) name(s) are used by more than one package:"
                $detail
                'All examples share one output directory, so these overwrite each other. Rename one side to a name that'
                'says what distinguishes it, or give it an explicit `[[example]] name = ...` in its Cargo.toml.'
            ) -join [Environment]::NewLine

            throw $message
        }
    }
}
