# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Guided release for every workspace package with unreleased modifications.

.DESCRIPTION
    Counterpart to release-packages.ps1 for the common case where the user
    knows that "something changed" but doesn't yet know exactly which packages
    need releasing. This script walks the user through every workspace package
    that has unreleased modifications (changes newer than its last
    `version =` / `publish =` commit) and asks, for each one:

      - view the diff,
      - ignore this package (treat the change as immaterial / not yet ready),
      - release as breaking / non-breaking / patch.

    Conceptually, this is equivalent to imagining a virtual `*` package that
    depends on every changed workspace package and using `release-packages.ps1`
    to release the cascade from `*`. There is no real `*` token; instead the
    review loop seeds the BFS with every changed package as an additional
    root.

    Every per-package decision triggers the same plan resolution + cascade
    that release-packages.ps1 uses. Acceptances become release-set members
    and cascade onto their dependents as usual; ignores leave the package
    untouched. Once the user has made a decision for every surfaced package,
    the resolved plan is executed atomically — Cargo.toml + workspace
    Cargo.toml + CHANGELOG.md + README.md updates are written, a workspace
    `cargo check` runs, and a summary is printed.

    This script is interactive-only. For scripted / CI use, invoke
    release-packages.ps1 with an explicit -Packages list so the choices are
    auditable.

.EXAMPLE
    # Walk through every changed package and decide for each whether to
    # release it (and in what change type) or skip it.
    .\release-changed-packages.ps1
#>
[CmdletBinding()]
param()

# All helpers, configuration, and Invoke-ReleaseChangedPackagesMain live in
# the library so this script stays a thin CLI shell. The library also
# dot-sources scripts/lib/releasing.ps1 transitively, so consumers only need
# this one import.
. "$PSScriptRoot/lib/release-flow.ps1"

Invoke-ReleaseChangedPackagesMain | Out-Null
