# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Updates the version of a Rust package and generates a CHANGELOG.md file based on git history.

.DESCRIPTION
    This script automates the full release of a Rust package in a workspace repository:
    1. Version Update: Either increment the version according to the kind of change being released
       (Breaking / NonBreaking / Patch), graduate a 0.x package to its first stable 1.0.0, or set
       a specific version explicitly. Cargo's 0.x.y SemVer rules are honored — for `0.x.y` packages
       a Breaking change becomes `0.(x+1).0` and both NonBreaking and Patch map to incrementing `y`.
    2. Cascade: Every workspace package that depends on the target via `[dependencies]` or
       `[build-dependencies]` (transitively) is also released. The kind of change applied to each
       dependent is informed by `[package.metadata.cargo_check_external_types]` AND by whether
       the target's change is SemVer-incompatible under Cargo's rules:
         * If the dependent exposes any type rooted at the released package in its public API
           (or does not declare allowed_external_types at all), the dependent gets a breaking
           change when the target's change is breaking (e.g. `0.0.x → 0.0.(x+1)`, `0.x.y → 0.(x+1).0`,
           `1.x → 2.0`); otherwise the same kind as the target. This ensures the dependent's
           own version increment reflects the breaking change in its public API surface.
         * Otherwise, the dependent only uses the released package internally, and a patch
           change is applied: enough to refresh the workspace-pinned version, but without
           overstating the change to downstream consumers.
       Dev-only dependents are skipped — they automatically pick up the new workspace version.
    3. Changelog Generation: A CHANGELOG.md entry is generated for the target and every cascaded
       dependent. Cascaded packages that have no other commits since their last release get a single
       `Now requires <new-version> of \`<target>\`` entry under `🔧 Maintenance` (or `⚠️ Breaking`
       for breaking changes).

    By default, if neither --version nor --change is specified, the script performs a NonBreaking
    release of the target package (e.g., 1.2.3 -> 1.3.0, or 0.3.3 -> 0.3.4 for `0.x.y` packages).

.PARAMETER Name
    The name of the package to release. This should match the folder name inside the 'crates'
    directory. The aliases `-CrateName` and `-PackageName` are accepted for backward compatibility
    with prior iterations of this script.

.PARAMETER Version
    [Optional] The specific version to set (e.g., "1.2.3"). Can be specified with --version or -v.
    This parameter is mutually exclusive with --change.

.PARAMETER Change
    [Optional] The kind of change being released. Releasers reason in semantic terms — this
    parameter accepts those terms directly and the script translates them into the appropriate
    Cargo version transition based on the package's current version. Can be specified with
    --change or -c. This parameter is mutually exclusive with --version.

    Accepted values:
    - Breaking:    SemVer-incompatible change. 1.x.y -> (x+1).0.0; 0.x.y -> 0.(x+1).0; 0.0.x -> 0.0.(x+1).
    - NonBreaking: SemVer-compatible feature or addition. 1.x.y -> x.(y+1).0; 0.x.y -> 0.x.(y+1).
    - Patch:       SemVer-compatible internal change with no API impact (typically a bug fix
                   or any other change that doesn't affect what downstream consumers can do).
                   x.y.z -> x.y.(z+1).
    - 1.0:         One-time graduation event for a 0.x package to its first stable 1.0.0. Errors
                   out when the package is already at or beyond 1.0.0. Cascades as a Breaking
                   change.

.EXAMPLE
    # Default behavior — non-breaking release of 'my-package'
    .\release-crate.ps1 "my-package"

.EXAMPLE
    # Set a specific version for 'my-package'
    .\release-crate.ps1 my-package --version "2.5.0"

.EXAMPLE
    # Release 'my-package' as a breaking change
    .\release-crate.ps1 my-package --change Breaking

.EXAMPLE
    # Release 'my-package' as a patch
    .\release-crate.ps1 my-package -c Patch

.EXAMPLE
    # Graduate 'my-package' from 0.x to 1.0.0
    .\release-crate.ps1 my-package --change 1.0
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [Alias('CrateName', 'PackageName')]
    [string]$Name,

    [Parameter(Mandatory = $false)]
    [Alias('v')]
    [string]$Version,

    [Parameter(Mandatory = $false)]
    [Alias('c')]
    [ValidateSet('Breaking', 'NonBreaking', 'Patch', '1.0')]
    [string]$Change,

    # Base ref used to identify the release set (packages whose `version =` differs
    # between this ref and HEAD) for the post-release upstream-dependency scan.
    # The modification baseline for each upstream dep is per-package (the dep's own
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

Invoke-ReleaseMain -PackageName $Name -Version $Version -Change $Change -BaseRef $BaseRef -NonInteractive:$NonInteractive | Out-Null
