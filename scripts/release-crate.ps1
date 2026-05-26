# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates the version of a Rust crate and generates a CHANGELOG.md file based on git history.

.DESCRIPTION
    This script automates the full release of a Rust crate in a workspace repository:
    1. Version Bump: Automatically increment the version (major, minor, or patch) or set a specific
       version. Cargo's 0.x.y SemVer rules are honored — for `0.x.y` crates a `major` bump becomes
       `0.(x+1).0` and both `minor` and `patch` map to bumping `y`.
    2. Cascade: Every workspace crate that depends on the target via `[dependencies]` or
       `[build-dependencies]` (transitively) is also bumped. The bump kind applied to each
       dependent is informed by `[package.metadata.cargo_check_external_types]` AND by whether
       the target's bump is SemVer-incompatible under Cargo's rules:
         * If the dependent exposes any type rooted at the bumped crate in its public API
           (or does not declare allowed_external_types at all), the dependent gets a `major`
           bump when the target's bump is breaking (e.g. `0.0.x → 0.0.(x+1)`, `0.x.y → 0.(x+1).0`,
           `1.x → 2.0`); otherwise the same kind as the target. This ensures the dependent's
           own version increment reflects the breaking change in its public API surface.
         * Otherwise, the dependent only uses the bumped crate internally, and a `patch` bump
           is applied: enough to refresh the workspace-pinned version, but without overstating
           the change to downstream consumers.
       Dev-only dependents are skipped — they automatically pick up the new workspace version.
    3. Changelog Generation: A CHANGELOG.md entry is generated for the target and every cascaded
       dependent. Cascaded crates that have no other commits since their last release get a single
       `bump \`<target>\` to <new-version>` entry under `🔧 Maintenance` (or `⚠️ Breaking`
       for major bumps).

    By default, if neither --version nor --bump is specified, the script will perform a minor bump
    of the target crate (e.g., 1.2.3 -> 1.3.0, or 0.3.3 -> 0.3.4 for `0.x.y` crates).

.PARAMETER CrateName
    The name of the crate to release. This should match the folder name inside the 'crates' directory.

.PARAMETER Version
    [Optional] The specific version to set (e.g., "1.2.3"). Can be specified with --version or -v.
    This parameter is mutually exclusive with --bump.

.PARAMETER Bump
    [Optional] The version component to bump: 'major', 'minor', or 'patch'. Can be specified with --bump or -b.
    - major: Increments the major version and resets minor and patch to 0 (e.g., 1.2.3 -> 2.0.0)
    - minor: Increments the minor version and resets patch to 0 (e.g., 1.2.3 -> 1.3.0)
    - patch: Increments the patch version (e.g., 1.2.3 -> 1.2.4)
    This parameter is mutually exclusive with --version.

.EXAMPLE
    # Increment the minor version for 'my-crate' (default behavior)
    .\release-crate.ps1 "my-crate"

.EXAMPLE
    # Set a specific version for 'my-crate'
    .\release-crate.ps1 my-crate --version "2.5.0"

.EXAMPLE
    # Bump the major version for 'my-crate'
    .\release-crate.ps1 my-crate --bump major

.EXAMPLE
    # Bump the patch version for 'my-crate'
    .\release-crate.ps1 my-crate -b patch
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$CrateName,

    [Parameter(Mandatory = $false)]
    [Alias('v')]
    [string]$Version,

    [Parameter(Mandatory = $false)]
    [Alias('b')]
    [ValidateSet('major', 'minor', 'patch')]
    [string]$Bump,

    # Base ref used to identify the release set (crates whose `version =` differs
    # between this ref and HEAD) for the post-release upstream-dependency scan.
    # The modification baseline for each upstream dep is per-crate (the dep's own
    # last `version =` / `publish =` commit), not this ref. Default is
    # 'origin/main' (best-effort fetched before use). Pass an empty string to skip
    # the scan entirely.
    [Parameter(Mandatory = $false)]
    [string]$BaseRef = 'origin/main',

    # Suppress all interactive prompts (decline-by-default for the upstream-dependency
    # scan). Auto-enabled in CI / when stdin is redirected; this switch is the explicit
    # override for scripted callers.
    [Parameter(Mandatory = $false)]
    [switch]$NonInteractive
)

# All helpers, configuration, and Invoke-ReleaseMain live in the library so this
# script stays a thin CLI shell. The library also dot-sources scripts/lib/releasing.ps1
# transitively, so consumers only need this one import.
. "$PSScriptRoot/lib/release-flow.ps1"

Invoke-ReleaseMain -CrateName $CrateName -Version $Version -Bump $Bump -BaseRef $BaseRef -NonInteractive:$NonInteractive | Out-Null
