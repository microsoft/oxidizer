# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Releases one or more workspace packages from a single bundled plan.

.DESCRIPTION
    Drives the full release of every package listed in -Packages plus every
    workspace package that needs to cascade as a consequence:
    1. Plan resolution: parse every token, compute the transitive
       cascade-toward-dependents from each user-listed package, fold in
       per-package change-type requirements (the user's request, auto-upgraded
       when the cascade requires a stronger change).
    2. Pre-release review: any workspace package that has unreleased
       modifications (changes newer than its last `version =` / `publish =`
       commit) and is not already part of the resolved plan is surfaced to
       the user one at a time so the user can elevate-into / decline-from the
       plan. Plan-resolution re-runs after every decision. Release-set
       members carrying a non-breaking cascade-applied change type are also
       surfaced so the user can decide whether to elevate them based on
       their pending modifications.
    3. Plan display: the final resolved plan is printed to the console for a
       last eyeball before any file writes happen.
    4. Atomic execution: the plan is executed top-down — Cargo.toml +
       workspace Cargo.toml + CHANGELOG.md + README.md are updated for every
       release-set member in topological order (dependencies first,
       dependents last). No prompts are issued during execution; every
       decision was made in step 2.
    5. Post-execution: a workspace `cargo check` confirms the workspace still
       builds, and a summary + next-steps message lists every released
       package with its old → new version.

    Cargo's 0.x.y SemVer rules are honored throughout: for `0.x.y` packages a
    Breaking change becomes `0.(x+1).0`, NonBreaking and Patch both map to
    incrementing `y`. The dependent cascade also respects
    `[package.metadata.cargo_check_external_types]`: a dependent that does
    not expose any type rooted at the released package cascades as a `patch`
    rather than mirroring the target's change type. Dev-only dependents are
    skipped — they automatically pick up the new workspace version.

.PARAMETER Packages
    The list of workspace packages to release, in the form
    `<name>@<change-spec>`. Names match the folder name under `crates/` (or
    the Cargo package name if it differs by `_`/`-`). Accepted change specs:

    - `breaking`         : SemVer-incompatible change. 1.2.3 -> 2.0.0;
                           0.4.1 -> 0.5.0; 0.0.5 -> 0.0.6.
    - `nonbreaking`      : SemVer-compatible feature/addition.
                           1.2.3 -> 1.3.0; 0.4.1 -> 0.4.2; 0.0.5 -> 0.0.6.
    - `patch`            : SemVer-compatible internal change. 1.2.3 -> 1.2.4;
                           0.4.1 -> 0.4.2 (numerically equal to nonbreaking
                           on 0.x.y packages).
    - `<major>.<minor>.<patch>` : explicit version pin, e.g. `1.0.0` (also
                           used for the one-time 0.x -> 1.0.0 graduation
                           event).

    Each release decision is a judgment call: the author must review the
    actual diff being released (source + dependency edits) and decide
    whether the cumulative change is a breaking SemVer change, a backward-
    compatible addition, a pure internal patch, or an explicit version pin.
    Picking too weak a change type causes dependents to silently get
    incompatible behaviour after `cargo update`; picking too strong is
    harmless except it forces direct dependents to bump as well.

.EXAMPLE
    # Release 'bytesbuf_io' as a breaking change. Cascade is automatic.
    .\release-packages.ps1 -Packages 'bytesbuf_io@breaking'

.EXAMPLE
    # Release 'bytesbuf' and 'http_extensions' in a single transaction:
    # bytesbuf as breaking, http_extensions as non-breaking. Any cascade
    # between them or onto their dependents is computed automatically.
    .\release-packages.ps1 -Packages 'bytesbuf@breaking','http_extensions@nonbreaking'

.EXAMPLE
    # Pin a specific version (e.g. graduate 'my-package' from 0.x to 1.0.0)
    .\release-packages.ps1 -Packages 'my-package@1.0.0'
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateNotNull()]
    [string[]]$Packages
)

# All helpers, configuration, and Invoke-ReleasePackagesMain live in the
# library so this script stays a thin CLI shell. The library also dot-sources
# scripts/lib/releasing.ps1 transitively, so consumers only need this one
# import.
. "$PSScriptRoot/lib/release-flow.ps1"

Invoke-ReleasePackagesMain -Packages $Packages | Out-Null
