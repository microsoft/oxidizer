# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

#Requires -Version 7.0

<#
.SYNOPSIS
    Releases one or more workspace packages from a single bundled plan.

.DESCRIPTION
    The driver supports three mutually-exclusive modes for selecting which
    workspace packages to release. In every mode the same downstream pipeline
    runs: plan resolution + cascade toward dependents, an elevation review for
    any modified-but-unreleased dependencies, a final plan display, and atomic
    execution of all Cargo.toml / CHANGELOG.md / README.md / workspace
    Cargo.toml writes, followed by a workspace `cargo check` and a summary.

    Every mode is interactive — even the targeted mode prompts for elevation
    review when modified-but-unreleased dependencies of the requested packages
    are detected. The script must be run from an interactive terminal.

    Modes:

    1. Targeted (-Packages, default).
       The caller provides the entire release plan up front as a list of
       `<name>@<change-spec>` tokens. The planner cascades toward dependents
       and surfaces any modified-and-unreleased dependencies for review.

    2. Changed (-Changed).
       Guided walk: the planner scans the workspace for every package with
       unreleased modifications (changes newer than its last `version =` /
       `publish =` commit) and walks the user through them one prompt at a
       time. For each surfaced package the user can view the diff, skip the
       package, or release it as breaking / non-breaking / patch. Each
       acceptance is fed back to the planner, which re-resolves the release
       set and cascade so the next iteration surfaces only newly-relevant
       elevation candidates.

       The change scan only sees files under `crates/<package>/`. Modifications
       to anything outside a package directory — for example the workspace-level
       `Cargo.toml`, `.cargo/`, `deny.toml`, or shared CI configuration — do
       NOT surface a package as "modified" even if they affect how the package
       builds or behaves. If you suspect such a cross-cutting change matters,
       use `-All` (which walks every publishable package regardless of detected
       changes) or list the affected packages explicitly via `-Packages`.

    3. All (-All).
       Same guided walk as -Changed, but the change-detection scan is
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

    User-provided change types may be automatically upgraded by cascade
    analysis if dependency exposure rules require a stronger change type
    (e.g. an exposing dependent of a breaking release is upgraded from
    your requested `patch` to `breaking`). If an explicit version number
    is specified for a package and cascade logic requires a higher
    version number than the pin allows, the release plan is rejected
    (or, with -Force, the pin is honored verbatim and a warning is
    printed flagging that downstream consumers may break).

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
    - `<major>.<minor>.<patch>[-<prerelease>][+<build>]` : explicit SemVer 2.0
                           version pin. Must have exactly three numeric
                           components — 1- or 2-component forms like `1` or
                           `1.2` are rejected. Examples: `1.0.0`, `2.5.0`,
                           `1.0.0-rc.1`, `0.1.0-pre01`, `1.0.0-beta+meta`.
                           Must be strictly greater than the package's
                           current on-disk version per SemVer 2.0 ordering
                           (so e.g. `1.0.0-rc.1` < `1.0.0`).

    Each release decision is a judgment call: the author must review the
    actual diff being released (source + dependency edits) and decide
    whether the cumulative change is a breaking SemVer change, a backward-
    compatible addition, a pure internal patch, or an explicit version pin.
    Picking too weak a change type causes dependents to silently get
    incompatible behaviour after `cargo update`; picking too strong is
    harmless except it forces direct dependents to bump as well.

.PARAMETER Changed
    Switch: walk through every workspace package that has unreleased
    modifications (changes newer than its last `version =` / `publish =`
    commit) and prompt for a per-package release decision. Mutually
    exclusive with -Packages and -All.

    The change scan only sees files under `crates/<package>/`; it cannot
    detect impactful changes elsewhere in the repository (e.g. the
    workspace-level `Cargo.toml`, `.cargo/`, `deny.toml`, or shared CI
    configuration). If a cross-cutting change matters, use -All instead or
    pass the affected packages explicitly via -Packages.

.PARAMETER All
    Switch: walk through every publishable workspace package, even ones
    with no on-disk modifications. Use to force-walk the workspace when you
    need a coordinated multi-package release plan or when a refactor might
    have touched packages the modification scan misses. Mutually exclusive
    with -Packages and -Changed.

.EXAMPLE
    # Release 'bytesbuf_io' as a breaking change. Cascade is automatic.
    .\release-packages.ps1 -Packages 'bytesbuf_io@breaking'

.EXAMPLE
    # Release 'bytesbuf' and 'http_extensions' in a single transaction:
    # bytesbuf as breaking, http_extensions as non-breaking. Any cascade
    # between them or onto their dependents is computed automatically.
    .\release-packages.ps1 -Packages 'bytesbuf@breaking','http_extensions@nonbreaking'

.PARAMETER Force
    Switch (valid only with -Packages): relax the explicit-version-pin
    rejection. By default, if a cascade computation requires a higher
    version than an explicit `<name>@<major>.<minor>.<patch>` pin
    allows, the release plan is rejected (the script refuses to
    silently override an explicit pin). With -Force, the explicit pin
    is honored verbatim, the package's EffectiveChangeType tag is
    upgraded to match the cascade so any further cascade decisions are
    correct, and a warning is printed flagging that downstream
    consumers may break.

    -Force does NOT relax the always-fatal "pin is not strictly greater
    than the current on-disk version" check, and has no effect on
    change-type tokens (which are always auto-upgraded silently).

    -Force is not exposed in -Changed or -All mode: those modes only
    accept change-type answers (breaking / non-breaking / patch) and
    never explicit version pins, so the pin-vs-cascade rejection
    cannot fire there.

.EXAMPLE
    # Pin a specific version, e.g. release 'my-package' as 1.0.0.
    .\release-packages.ps1 -Packages 'my-package@1.0.0'

.EXAMPLE
    # Pin a pre-release version.
    .\release-packages.ps1 -Packages 'my-package@1.0.0-rc.1'

.EXAMPLE
    # Force-honor a pin even when cascade analysis requires a higher version
    # (downstream consumers may break — use with caution).
    .\release-packages.ps1 -Packages 'my-package@1.0.0' -Force

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
    [switch]$All,

    [Parameter(ParameterSetName = 'ByPackages')]
    [switch]$Force
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

Invoke-ReleasePackagesMain -Mode $mode -Packages $Packages -Force:$Force | Out-Null
