# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Releases one or more workspace packages from a single bundled plan.

.DESCRIPTION
    The driver supports three mutually-exclusive modes for selecting which
    workspace packages to release. In every mode the same downstream pipeline
    runs: plan resolution + cascade toward dependents, an interactive
    elevation review for any modified-but-unreleased dependencies, a final
    plan display, and atomic execution of all Cargo.toml / CHANGELOG.md /
    README.md / workspace Cargo.toml writes, followed by a workspace
    `cargo check` and a summary.

    Modes:

    1. Targeted (-Packages, default).
       The caller provides the entire release plan up front as a list of
       `<name>@<change-spec>` tokens. The planner cascades toward dependents
       and surfaces any modified-and-unreleased dependencies for review. This
       is the only mode that works non-interactively.

    2. Changed (-Changed).
       Interactive guided walk: the planner scans the workspace for every
       package with unreleased modifications (changes newer than its last
       `version =` / `publish =` commit) and walks the user through them one
       prompt at a time. For each surfaced package the user can view the diff,
       ignore the package, or release it as breaking / non-breaking / patch.
       Each acceptance is fed back to the planner, which re-resolves the
       release set and cascade so the next iteration surfaces only newly-
       relevant elevation candidates.

    3. All (-All).
       Same interactive walk as -Changed, but the change-detection scan is
       skipped: every publishable workspace package is surfaced for review,
       even ones with no on-disk modifications. Use this when you want to
       force-walk the entire workspace (e.g. preparing a coordinated multi-
       package release after a refactor that may have touched everything).
       Surfaced packages with no detected changes still expose the View-diff
       menu option (relabelled to make the empty state obvious).

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
    - `<major>.<minor>.<patch>` : explicit version pin, e.g. `1.0.0` or
                           `2.5.0`. Must be strictly greater than the
                           package's current on-disk version.

    Each release decision is a judgment call: the author must review the
    actual diff being released (source + dependency edits) and decide
    whether the cumulative change is a breaking SemVer change, a backward-
    compatible addition, a pure internal patch, or an explicit version pin.
    Picking too weak a change type causes dependents to silently get
    incompatible behaviour after `cargo update`; picking too strong is
    harmless except it forces direct dependents to bump as well.

.PARAMETER Changed
    Interactive switch: walk through every workspace package that has
    unreleased modifications (changes newer than its last `version =` /
    `publish =` commit) and prompt for a per-package release decision.
    Mutually exclusive with -Packages and -All. Interactive-only — refuses
    to run when stdin is not a terminal.

.PARAMETER All
    Interactive switch: walk through every publishable workspace package,
    even ones with no on-disk modifications. Use to force-walk the workspace
    when you need a coordinated multi-package release plan or when a refactor
    might have touched packages the modification scan misses. Mutually
    exclusive with -Packages and -Changed. Interactive-only.

.EXAMPLE
    # Release 'bytesbuf_io' as a breaking change. Cascade is automatic.
    .\release-packages.ps1 -Packages 'bytesbuf_io@breaking'

.EXAMPLE
    # Release 'bytesbuf' and 'http_extensions' in a single transaction:
    # bytesbuf as breaking, http_extensions as non-breaking. Any cascade
    # between them or onto their dependents is computed automatically.
    .\release-packages.ps1 -Packages 'bytesbuf@breaking','http_extensions@nonbreaking'

.EXAMPLE
    # Pin a specific version, e.g. release 'my-package' as 1.0.0.
    .\release-packages.ps1 -Packages 'my-package@1.0.0'

.EXAMPLE
    # Guided walk through every workspace package with unreleased modifications.
    .\release-packages.ps1 -Changed

.EXAMPLE
    # Guided walk through every publishable workspace package.
    .\release-packages.ps1 -All
#>
[CmdletBinding(DefaultParameterSetName = 'ByPackages')]
param(
    [Parameter(Mandatory = $true, Position = 0, ParameterSetName = 'ByPackages')]
    [ValidateNotNull()]
    [string[]]$Packages,

    [Parameter(Mandatory = $true, ParameterSetName = 'Changed')]
    [switch]$Changed,

    [Parameter(Mandatory = $true, ParameterSetName = 'All')]
    [switch]$All
)

# All helpers, configuration, and Invoke-ReleasePackagesMain live in the
# library so this script stays a thin CLI shell. The library also dot-sources
# scripts/lib/releasing.ps1 transitively, so consumers only need this one
# import.
. "$PSScriptRoot/lib/release-flow.ps1"

$mode = switch ($PSCmdlet.ParameterSetName) {
    'ByPackages' { 'targeted' }
    'Changed'    { 'changed' }
    'All'        { 'all' }
}

Invoke-ReleasePackagesMain -Mode $mode -Packages $Packages | Out-Null
