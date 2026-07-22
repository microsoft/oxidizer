# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

#
# Each check belongs to one of four buckets, which determines how it
# interprets the impact env vars emitted by the cargo-delta impact step
# in cloud workflows:
#
#   - "modified": only run when at least one package's source files
#     changed in the diff. The check's underlying tool is workspace-wide
#     or directory-scoped (cargo fmt --all, cargo heather, cargo
#     spellcheck), so it doesn't take --package; we short-circuit on
#     the ANVIL_INCLUDE_MODIFIED == "--skip" sentinel.
#
#   - "affected": run on the affected set (modified ∪ reverse-deps
#     within the workspace). The check's underlying tool takes
#     --package; we splice ANVIL_INCLUDE_AFFECTED into the cargo
#     invocation, defaulting to --workspace for local invocations where
#     no env var is set.
#
#   - "required": run on the required set (affected ∪ workspace-internal
#     transitive deps). Same splice/default pattern as affected, but
#     keyed on ANVIL_INCLUDE_REQUIRED. Used for checks whose tool
#     resolves through the dep graph (cargo doc → intra-doc links;
#     cargo hack → feature powerset; cargo udeps → unused-deps).
#
#   - "unscoped": always run, no env var reference. External-input
#     checks (deny, audit, aprz) and PR-context checks (pr-title) live
#     here. Scheduled-exhaustive recipes (mutants-full) are also unscoped
#     by design.
#
# Local invocation (no impact wiring): all three env vars are unset or
# empty (recipes fall back to "--workspace" via the truthiness check
# below); modified-tier recipes simply skip the splice and run their
# workspace-wide tool; affected/required-tier recipes splat
# "--workspace" when the env var is empty or unset.
#
# Preparation contract: when a recipe reaches the cargo call, the
# env var is one of:
#
#   * unset or ""                 - local run, or a cloud workflow (e.g.
#                                    scheduled) that leaves the tier
#                                    unscoped; the recipe substitutes
#                                    "--workspace". An empty string is
#                                    NOT $null, so we test truthiness
#                                    rather than `?? "--workspace"`
#                                    (which would only catch $null and
#                                    splat an empty arg cargo rejects).
#   * "--package A@v --package B@v"  - emitted by the cloud-workflow impact step
#                                    when the tier has members. Packages are
#                                    version-qualified cargo specs so they
#                                    resolve uniquely even when a like-named
#                                    crate is also a transitive dependency.
#   * "--skip"                    - emitted by the cloud-workflow impact step when
#                                    the tier is empty (recipe exits 0)
#
# This lets the simple recipes splat the var directly with
#   & cargo X @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) ...
# and reduces the per-recipe boilerplate to a single one-line skip
# guard plus the cargo invocation.
#
# Modified-tier recipes never splice the env var into cargo (their
# tools are workspace-wide); they only check the skip sentinel.
#
# Every recipe whose body uses multi-line conditionals or env-var
# splicing is annotated with [script("pwsh")]. pwsh is preinstalled on
# Windows (since Windows 10), on GH/ADO hosted Linux + Windows
# runners, and installable on macOS via Homebrew or the upstream
# installer. We chose pwsh over bash because just's shebang dispatch
# requires `cygpath` on Windows (only on PATH from inside Git Bash),
# while [script("pwsh")] works from plain PowerShell with no PATH
# augmentation. We require pwsh 7+ (enforced via _anvil-require pwsh)
# for consistent semantics of the array-splat splice and the modern
# operators used across these recipes.
#
# Every recipe that runs a command is annotated with [script("pwsh")],
# including single-command ones (cargo deny check, cargo audit). They do
# NOT inherit the adopter's default shell: just defaults to `sh`, which is
# absent on Windows and varies across environments, so relying on it makes
# these recipes non-portable. Routing every command through pwsh keeps
# behavior identical on every platform.
#
# Note: [script(...)] requires `set unstable`. The adopter's root
# justfile must declare it (typically as a top-level line). anvil's
# mod.just does NOT redeclare it, to avoid conflicting with adopters
# who already have it.

# _anvil-base-ref: single source of truth for "which git ref does a PR
# diff against". Shared by the impact step (github/impact-action.yml,
# ado/steps/impact.yml) and anvil-mutants-diff so the resolution lives in
# exactly one place. Precedence:
#   1. BASE_REF -- explicit override. GitHub sets it to the PR base commit
#      SHA; adopters can set a full ref like `origin/release`.
#   2. SYSTEM_PULLREQUEST_TARGETBRANCH -- ADO PR builds. This is the
#      *fully-qualified* target ref (e.g. `refs/heads/main`), so strip the
#      `refs/heads/` prefix and resolve to `origin/<branch>`
#      (`origin/refs/heads/main` is not a valid ref).
#   3. GITHUB_BASE_REF -- GitHub PR builds. Unqualified target branch name.
#   4. origin/main, then origin/master -- local / non-PR fallback.
# Prints ONLY the resolved ref to stdout so callers can capture it:
#   pwsh : $base = (& {{just_executable()}} _anvil-base-ref)
#   bash : base="$(just _anvil-base-ref)"
[script("pwsh")]
_anvil-base-ref:
    $ErrorActionPreference = 'Stop'
    if ($env:BASE_REF) {
        Write-Output $env:BASE_REF
        exit 0
    }
    if ($env:SYSTEM_PULLREQUEST_TARGETBRANCH) {
        Write-Output ("origin/" + ($env:SYSTEM_PULLREQUEST_TARGETBRANCH -replace '^refs/heads/', ''))
        exit 0
    }
    if ($env:GITHUB_BASE_REF) {
        Write-Output "origin/$($env:GITHUB_BASE_REF)"
        exit 0
    }
    foreach ($candidate in @('origin/main', 'origin/master')) {
        git rev-parse --verify $candidate 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Write-Output $candidate
            exit 0
        }
    }
    Write-Error 'anvil-base-ref: cannot resolve a base ref. Set BASE_REF, or ensure origin/main or origin/master exists.'
    exit 1

# _anvil-impact-format: single source of truth for turning a cargo-delta
# impact report into the `--package <spec> ...` string the checks.just
# recipes consume. Shared by both impact steps
# (github/impact-action.yml, ado/steps/impact.yml) so the formatting
# logic lives in exactly one place rather than being duplicated (and
# subtly diverging) across two shell dialects.
#
# Arguments:
#   tier       - one of "modified" / "affected" / "required". Selected
#                from the cargo-delta JSON (which uses TitleCase keys).
#   impactJson - path to the file the caller produced via
#                `cargo delta impact --format json`.
#
# Output (stdout, exactly one line so callers can capture it):
#   * "--package name@version --package name2@version2 ..." when the
#     tier has members. Each package is a version-qualified cargo spec:
#     the version (read from `cargo metadata --no-deps`, i.e. workspace
#     members only) disambiguates a workspace member from a like-named
#     transitive dependency pulled in at a different version, which a
#     bare `-p name` cannot do ("multiple packages named name").
#   * "--skip" when the tier is empty (recipes treat this as a no-op).
# Diagnostics (dropped unknown names) go to STDERR so they never
# pollute the captured value.
#
# Why pwsh + ConvertFrom-Json rather than jq + read: cargo-delta and
# cargo-metadata both emit JSON, and parsing structured data through a
# tab-delimited `IFS` read is fragile -- e.g. tab is an IFS-whitespace
# character, so an empty interior field (a package with no lib target)
# coalesces with its delimiters and silently shifts every later field.
# Working with objects sidesteps that entire class of bug.
#
# Usage:
#   bash : val="$(just _anvil-impact-format affected "$impact_json")"
#   pwsh : $val = (& {{just_executable()}} _anvil-impact-format affected $impactJson)
[script("pwsh")]
_anvil-impact-format tier impactJson:
    $ErrorActionPreference = 'Stop'
    $tier = '{{tier}}'
    $impactJson = '{{impactJson}}'
    # cargo-delta keys its impact tiers in TitleCase.
    $tierKey = switch ($tier) {
        'modified' { 'Modified' }
        'affected' { 'Affected' }
        'required' { 'Required' }
        default {
            Write-Error "anvil impact: unknown tier '$tier' (expected modified/affected/required)"
            exit 1
        }
    }
    # Build two lookups from workspace members:
    #   * $byName: exact cargo package name (may contain hyphens) -> pkg
    #   * $byLib:  library target name (snake_case) -> pkg
    # cargo-delta reports *library* names (snake_case), but for crates
    # whose package name differs from the lib name (e.g. `cargo-anvil`
    # vs `cargo_anvil`) we must translate back to the package name cargo
    # accepts. Bin-only crates have no lib target; cargo-delta reports
    # those by their package name, caught by $byName.
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $byName = @{}
    $byLib = @{}
    foreach ($p in $meta.packages) {
        $byName[$p.name] = $p
        foreach ($t in $p.targets) {
            if ($t.kind -contains 'lib') { $byLib[$t.name] = $p }
        }
    }
    $impact = Get-Content -LiteralPath $impactJson -Raw | ConvertFrom-Json
    $names = @($impact.$tierKey)
    $specs = [System.Collections.Generic.List[string]]::new()
    foreach ($name in $names) {
        if ([string]::IsNullOrWhiteSpace($name)) { continue }
        $p = if ($byName.ContainsKey($name)) {
            $byName[$name]
        } elseif ($byLib.ContainsKey($name)) {
            $byLib[$name]
        } else {
            $null
        }
        if ($null -eq $p) {
            # Names cargo-delta emits that aren't valid workspace
            # packages (e.g. directory-leaf ambiguities like `ffi` /
            # `ffi_build` in deeply nested workspaces) are dropped with a
            # warning rather than failing the whole build.
            [Console]::Error.WriteLine("anvil impact: dropping unknown package '$name' from $tier set")
            continue
        }
        $specs.Add("--package $($p.name)@$($p.version)")
    }
    if ($specs.Count -eq 0) {
        Write-Output '--skip'
    } else {
        Write-Output ($specs -join ' ')
    }
