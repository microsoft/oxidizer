# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../docs/design/checks.md for the catalog and rationale.
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

# === pr-fast members ====================================================

# Modified tier. cargo-fmt is a rustup component. We invoke it via the
# pinned nightly (see versions.just) because rustfmt.toml uses
# unstable_features = true (imports_granularity, group_imports,
# format_code_in_doc_comments). Floating nightly would mean
# format-drift breaking cloud workflows on rustup updates — the same trap we
# explicitly avoid for udeps/miri/careful/external-types.
[script("pwsh")]
anvil-fmt: anvil-fmt-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo '+{{ rust_nightly }}' fmt --all --check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Affected tier. Per project policy, clippy runs on the affected set
# rather than the modified set: a change in a crate can introduce a
# clippy issue in a dependent crate (e.g., trait-bound or
# obviously-truthy-condition lints that key off the changed type), so
# we want downstream rev-deps to lint as well. cargo-clippy is a
# rustup component; same reasoning as fmt for the require.
[script("pwsh")]
anvil-clippy: anvil-clippy-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    & cargo clippy @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) --all-targets --all-features --locked "--" '-D' 'warnings'
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Modified tier.
#
# `--check-format` makes the formatting diff fail the check, not just the
# sort order. cargo-sort 2.1.2+ downgraded formatting-only diffs to
# warnings by default (PR #129); without this flag a local 2.1.2+ install
# would pass on formatting drift that the pinned version still rejects.
# The flag exists in 2.0.2 too (where it is a no-op, since that version
# already errors on formatting), so it is safe across the supported range.
[script("pwsh")]
anvil-cargo-sort: anvil-cargo-sort-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo sort --workspace --check --check-format
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Modified tier.
[script("pwsh")]
anvil-license-headers: anvil-license-headers-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo heather
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Modified tier.
[script("pwsh")]
anvil-ensure-no-cyclic-deps: anvil-ensure-no-cyclic-deps-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo ensure-no-cyclic-deps
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Modified tier.
[script("pwsh")]
anvil-ensure-no-default-features: anvil-ensure-no-default-features-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') { exit 0 }
    cargo ensure-no-default-features
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Required tier. cargo doc resolves intra-doc links through the dep
# graph, so a dep changing its public API can break doc-build in a
# crate that wasn't itself modified.
[script("pwsh")]
anvil-doc-build: anvil-doc-build-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_REQUIRED -eq '--skip') { exit 0 }
    $env:RUSTDOCFLAGS = '-D warnings'
    & cargo doc @(if ($env:ANVIL_INCLUDE_REQUIRED) { -split $env:ANVIL_INCLUDE_REQUIRED } else { '--workspace' }) --all-features --no-deps
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Modified tier.
#
# cargo-doc2readme regenerates a crate's README.md from its rustdoc.
# Two extension points users frequently need:
#   * Workspace-level template (`crates/README.j2` or `README.j2` at repo
#     root): a Tera template applied to every crate's README. anvil
#     auto-detects it and passes `--template` to the per-crate runs.
#   * Per-crate opt-out: hand-crafted READMEs (e.g. a tool crate whose
#     README is more freeform than the lib docs) opt out by adding
#     `[package.metadata.ox-gen-readme]\ndisable = true` to their
#     Cargo.toml. anvil skips those crates.
#
# Bin-only crates have no library rustdoc to base a README on, so they
# are skipped as well (cargo doc2readme requires a library target).
# TODO(anvil-runner): even after a `cargo ox-run` helper absorbs the
# skip/splat preamble, this recipe stays multi-step: per-crate
# iteration over library targets, cargo-metadata-driven opt-outs
# (publish=false, [package.metadata.ox-gen-readme] disable), per-crate
# Push-Location into the crate dir (cargo-doc2readme is CWD-sensitive
# rather than --manifest-path-driven), and per-crate template-path
# resolution. The ANVIL_INCLUDE_MODIFIED value would still need to
# be intersected with the lib-crate set rather than splatted into cargo.
[script("pwsh")]
anvil-readme-check: anvil-readme-check-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') {
        Write-Host 'anvil-readme-check: no modified packages; skipping'
        exit 0
    }
    # Detect a workspace-level README template. Two conventional
    # locations: crates/README.j2 (cargo-workspaces idiom) or
    # README.j2 at repo root.
    $template = $null
    foreach ($candidate in 'crates/README.j2', 'README.j2') {
        if (Test-Path $candidate) { $template = (Resolve-Path $candidate).Path; break }
    }
    # Iterate library crates. Filter by impact set when set, then drop
    # bin-only crates and opt-outs.
    $pkg = @(if ($env:ANVIL_INCLUDE_MODIFIED) { -split $env:ANVIL_INCLUDE_MODIFIED } else { '--workspace' })
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $byName = @{}
    foreach ($p in $meta.packages) { $byName[$p.name] = $p }
    $candidates = if ($pkg -contains '--workspace') {
        @($meta.packages | ForEach-Object { $_.name })
    } else {
        $names = @()
        for ($i = 0; $i -lt $pkg.Count; $i++) {
            if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
                # Impact emits version-qualified specs (name@version);
                # strip the @version to recover the bare package name
                # this metadata lookup keys on.
                $names += ($pkg[$i + 1] -split '@', 2)[0]; $i++
            }
        }
        $names
    }
    $hadFailure = $false
    foreach ($name in $candidates) {
        $p = $byName[$name]
        if (-not $p) { continue }
        if (-not ($p.targets | Where-Object { $_.kind -contains 'lib' })) { continue }
        # Skip private crates (publish = false). They aren't released
        # and rarely have a polished README. Mirrors the
        # `cargo workspaces exec --ignore-private` idiom adopters
        # commonly use.
        if ($p.publish -is [array] -and $p.publish.Count -eq 0) {
            Write-Host "anvil-readme-check: $name (skipped: publish = false)"
            continue
        }
        $disabled = $false
        if ($p.metadata -and $p.metadata.'ox-gen-readme' -and $p.metadata.'ox-gen-readme'.disable) {
            $disabled = $true
        }
        if ($disabled) {
            Write-Host "anvil-readme-check: $name (opted out via [package.metadata.ox-gen-readme])"
            continue
        }
        Write-Host "anvil-readme-check: $name"
        # cargo doc2readme writes / compares relative to its CWD (not
        # --manifest-path), so chdir into the crate before invoking
        # --check. We also compute a per-crate relative path to the
        # workspace-level template so the same template file works for
        # every crate (parallels the cargo-workspaces idiom).
        $crateDir = Split-Path -Parent $p.manifest_path
        Push-Location $crateDir
        try {
            $relTemplate = if ($template) {
                Resolve-Path -Relative -LiteralPath $template
            } else {
                $null
            }
            $args = @('doc2readme', '--check')
            if ($relTemplate) { $args += @('--template', $relTemplate) }
            & cargo @args
            if ($LASTEXITCODE -ne 0) { $hadFailure = $true }
        } finally {
            Pop-Location
        }
    }
    if ($hadFailure) { exit 1 }

# Modified tier.
#
# cargo-spellcheck reads a Hunspell-compatible dictionary file at the
# path configured in spellcheck.toml (typically `extra_dictionaries =
# ["target/spelling.dic"]`). The convention used by the surveyed
# Microsoft Rust repos is to keep the *source* word list in a
# human-edited `.spelling` file at the repo root and preprocess it
# into the .dic format at check time (Hunspell .dic requires:
# alphabetical sort, blank/numeric lines removed, line-count header).
# We always (re)generate `target/spelling.dic` so that a spellcheck.toml
# pointing at it never fails on a missing file -- if `.spelling` is
# absent we emit an empty dictionary (count `0`, no words) rather than
# leaving the path dangling.
[script("pwsh")]
anvil-spellcheck: anvil-spellcheck-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_MODIFIED -eq '--skip') {
        Write-Host 'anvil-spellcheck: no modified packages; skipping'
        exit 0
    }
    $output_file = 'target/spelling.dic'
    $filtered_lines = @()
    if (Test-Path '.spelling') {
        $lines = Get-Content '.spelling' | Sort-Object
        $filtered_lines = @($lines | Where-Object { $_ -notmatch '^\d+$' -and $_ -ne '' })
    }
    $line_count = $filtered_lines.Count
    [System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($output_file)) | Out-Null
    @($line_count) + $filtered_lines | Set-Content $output_file
    # Pass --cfg explicitly when a spellcheck.toml exists at repo root,
    # otherwise cargo-spellcheck falls back to its built-in defaults and
    # ignores user-curated dictionaries (`extra_dictionaries`, custom
    # hunspell langs, etc.).
    if (Test-Path 'spellcheck.toml') {
        cargo spellcheck --cfg spellcheck.toml check --code 1
    } else {
        cargo spellcheck check --code 1
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Unscoped (PR title is not source-related).
#
# Validates that $env:PR_TITLE matches Conventional Commits when set;
# no-op when unset. Cloud workflows inject PR_TITLE explicitly: GH Actions
# from ${{ github.event.pull_request.title }}; ADO resolves it via the REST
# API in steps/group.yml (ADO has no PR-title predefined variable). Locally
# it skips silently when PR_TITLE is unset -- there is no reliable way to
# recover the PR title, so the recipe defers the check to cloud workflows.
# It does NOT tolerate a non-empty-but-wrong value: a misconfigured cloud
# variable (e.g. an unexpanded pipeline macro) is a real error and fails
# the check loudly rather than silently passing.
[script("pwsh")]
anvil-pr-title: anvil-pr-title-validate-prereqs
    $title = $env:PR_TITLE
    if (-not $title) {
        Write-Host 'anvil-pr-title: PR_TITLE env var not set; skipping (check runs in cloud workflows)'
        exit 0
    }
    if ($title -notmatch '^(feat|fix|chore|docs|refactor|test|build|cloud workflows|perf|revert)(\([^)]+\))?!?: .+') {
        Write-Error "PR title '$title' does not match Conventional Commits"
        exit 1
    }

# Unscoped (consults external advisory DB; reads Cargo.lock, not
# workspace members). Single command, but run under pwsh so it does not
# depend on the adopter's default shell (absent/varying on Windows).
[script("pwsh")]
anvil-deny: anvil-deny-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo deny check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Unscoped (consults external advisory DB; reads Cargo.lock).
[script("pwsh")]
anvil-audit: anvil-audit-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo audit
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Required tier. cargo-udeps detects unused dependencies by resolving
# the full crate graph and seeing which deps are referenced; that's
# precisely what the required tier is for. Pinned to the general
# nightly defined in versions.just.
#
# Run twice, because cargo-udeps only analyzes the targets it is told
# to, and each invocation catches a variant the other masks:
#   1. default targets (lib + bins): catches a dep in `[dependencies]`
#      that is referenced only by tests/benches/examples -- it belongs
#      in `[dev-dependencies]`. Under `--all-targets` such a dep looks
#      "used" (the test/bench/example target satisfies the lookup), so
#      this run is the only one that surfaces it.
#   2. `--all-targets`: catches unused `[dev-dependencies]` -- which the
#      default-targets run never compiles, so it is the only one that
#      surfaces those.
# A genuinely-unused dep is caught by both; the two together cover all
# of {unused dep, unused dev-dep, dep that should be a dev-dep}.
[script("pwsh")]
anvil-udeps: anvil-udeps-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_REQUIRED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_REQUIRED) { -split $env:ANVIL_INCLUDE_REQUIRED } else { '--workspace' })
    # Pass 1: default targets (lib + bins) -- surfaces [dependencies] that
    # are only used by tests/benches/examples (should be dev-deps).
    & cargo '+{{ rust_nightly }}' udeps @pkg --all-features
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    # Pass 2: --all-targets -- surfaces unused [dev-dependencies].
    & cargo '+{{ rust_nightly }}' udeps @pkg --all-features --all-targets
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Affected tier (compares published API of changed crates against the
# baseline; only changed crates' surface is at risk).
#
# Several real-world conditions produce errors that aren't actually
# SemVer violations:
#   - bin-only crates have no API to compare ("no library targets found").
#   - crates not yet published to crates.io ("not found in registry").
#   - crates where the published baseline lacks a lib target the
#     current source has (bin -> bin+lib transition).
# We pre-filter to library-bearing crates from cargo metadata, then
# run cargo-semver-checks per-package and tolerate the
# "no-comparable-baseline" failure modes.
#
# Findings policy: this recipe is *advisory*. Real SemVer findings do
# NOT fail the recipe -- breaking changes between unreleased commits
# are normal (the major-version bump happens at release time, not on
# every PR). Instead, when there are findings we write a markdown
# advisory body to `target/anvil/comments/semver.md`; when the
# tree is clean we remove that file. cloud-workflow wiring (GH:
# marocchino/sticky-pull-request-comment; ADO: pwsh + REST API)
# inspects the file after the recipe and upserts / clears a sticky
# PR comment accordingly. Local invocation gets the same file
# written under target/ for inspection.
#
# TODO(anvil-runner): even after a `cargo ox-run` helper absorbs the
# skip/splat preamble, this recipe stays multi-step: cargo-metadata
# filter to library crates (cargo-semver-checks --workspace fails on
# bin-only workspaces), intersect ANVIL_INCLUDE_AFFECTED with that
# set, then per-crate invocation with selective error tolerance for
# unpublished crates ("not found in registry") and bin->bin+lib
# transitions ("no library targets found").
[script("pwsh")]
anvil-semver-check: anvil-semver-check-validate-prereqs
    $ErrorActionPreference = 'Stop'
    $commentFile = 'target/anvil/comments/semver.md'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-semver-check: no affected packages; skipping'
        Remove-Item -LiteralPath $commentFile -ErrorAction SilentlyContinue
        exit 0
    }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Build the candidate package list. Always iterate per-package over
    # library crates only -- cargo-semver-checks --workspace would fail
    # on workspaces that contain bin-only crates ("no library targets
    # found"), and we want the same tolerance for unpublished / bin->lib-
    # transition crates regardless of whether we got here via impact-
    # scoping (cloud workflows) or full-workspace fallback (local).
    $libPkgs = @{}
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    foreach ($p in $meta.packages) {
        if ($p.targets | Where-Object { $_.kind -contains 'lib' }) {
            $libPkgs[$p.name] = $true
        }
    }
    if ($pkg -contains '--workspace') {
        $packages = @($libPkgs.Keys)
    } else {
        $packages = @()
        for ($i = 0; $i -lt $pkg.Count; $i++) {
            if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
                # Impact emits version-qualified specs (name@version);
                # strip the @version to recover the bare package name.
                $name = ($pkg[$i + 1] -split '@', 2)[0]
                if ($libPkgs[$name]) { $packages += $name }
                $i++
            }
        }
    }
    if ($packages.Count -eq 0) {
        Write-Host 'anvil-semver-check: no affected library crates; skipping'
        Remove-Item -LiteralPath $commentFile -ErrorAction SilentlyContinue
        exit 0
    }
    $findings = New-Object System.Collections.Generic.List[string]
    foreach ($p in $packages) {
        Write-Host "anvil-semver-check: $p"
        $output = (& cargo semver-checks --package $p 2>&1) | Out-String
        if ($LASTEXITCODE -ne 0) {
            if ($output -match 'not found in registry|no library targets found') {
                Write-Host "  $p has no comparable baseline; skipping (likely unpublished or bin->lib transition)" -ForegroundColor Yellow
            } else {
                Write-Host $output
                # Append a per-crate findings block. Using one-line-at-a-time
                # appends keeps the markdown free of pwsh backtick-escape
                # gymnastics (single-quoted literals + the natural `n join
                # produce clean LF newlines and unambiguous triple-backticks).
                $findings.Add('### `' + $p + '`') | Out-Null
                $findings.Add('') | Out-Null
                $findings.Add('```') | Out-Null
                foreach ($line in ($output.TrimEnd() -split "`r?`n")) {
                    $findings.Add($line.TrimEnd()) | Out-Null
                }
                $findings.Add('```') | Out-Null
                $findings.Add('') | Out-Null
            }
        }
    }
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $commentFile)) | Out-Null
    if ($findings.Count -gt 0) {
        # Body starts with an HTML-comment marker so the ADO wiring can
        # locate the existing thread on subsequent runs (ADO has no
        # native "sticky comment header"; the marker is invisible to
        # human readers). Marocchino on GH uses its own `header:` input
        # and ignores the marker, but having it in the body keeps a
        # single source of truth across backends.
        $lines = New-Object System.Collections.Generic.List[string]
        $lines.Add('<!-- anvil-semver -->') | Out-Null
        $lines.Add('## :warning: Potential breaking changes detected') | Out-Null
        $lines.Add('') | Out-Null
        $lines.Add('`cargo semver-checks` flagged the following on this PR. This is **informational** -- breaking changes between commits are expected; the major-version bump happens at release time, not on every PR.') | Out-Null
        $lines.Add('') | Out-Null
        foreach ($f in $findings) { $lines.Add($f) | Out-Null }
        $body = ($lines -join "`n") + "`n"
        Set-Content -LiteralPath $commentFile -Value $body -Encoding UTF8 -NoNewline
        Write-Host ''
        Write-Host "anvil-semver-check: wrote advisory comment to $commentFile (recipe exits 0; cloud workflows will upsert a PR comment)" -ForegroundColor Yellow
    } else {
        Remove-Item -LiteralPath $commentFile -ErrorAction SilentlyContinue
    }
    # Advisory: always exit 0. cloud-workflow wiring posts/clears the PR comment.
    exit 0

# Affected tier (lints public API of changed crates and rev-deps).
#
# cargo-check-external-types is per-manifest: no --package/--workspace,
# only --manifest-path. Iterate the affected library crates and run
# the tool once each, pointing at the crate's Cargo.toml. Bin-only
# crates have no public API surface and are skipped.
# TODO(anvil-runner): even after a `cargo ox-run` helper absorbs the
# skip/splat preamble, this recipe stays multi-step: cargo-check-
# external-types is per-manifest (no --package/--workspace), so we
# build a name->manifest map from cargo metadata, filter to lib crates,
# intersect with ANVIL_INCLUDE_AFFECTED, and call the tool once per
# crate. Hard-fails on errors (no tolerance, unlike semver-check).
[script("pwsh")]
anvil-external-types: anvil-external-types-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-external-types: no affected packages; skipping'
        exit 0
    }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Build pkg-name -> manifest-path map, restricted to library crates.
    $libPkgs = @{}
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    foreach ($p in $meta.packages) {
        if ($p.targets | Where-Object { $_.kind -contains 'lib' }) {
            $libPkgs[$p.name] = $p.manifest_path
        }
    }
    # Decide which packages to check.
    $packages = @()
    if ($pkg -contains '--workspace') {
        $packages = $libPkgs.Keys
    } else {
        for ($i = 0; $i -lt $pkg.Count; $i++) {
            if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
                # Impact emits version-qualified specs (name@version);
                # strip the @version to recover the bare package name
                # this manifest-path map keys on.
                $name = ($pkg[$i + 1] -split '@', 2)[0]
                if ($libPkgs.ContainsKey($name)) { $packages += $name }
                $i++
            }
        }
    }
    if ($packages.Count -eq 0) {
        Write-Host 'anvil-external-types: no affected library crates; skipping'
        exit 0
    }
    $failed = $false
    foreach ($p in $packages) {
        Write-Host "anvil-external-types: $p"
        # cargo-check-external-types requires nightly rustdoc (uses
        # unstable -Z flags) AND pins a specific rustdoc-types schema
        # version. We pin nightly narrowly to the schema this tool
        # version expects via `rust_nightly_external_types` in
        # versions.just — bump that pin alongside any cargo-check-
        # external-types upgrade. No tolerance for schema mismatches:
        # if it fails, the pin or the tool needs to move.
        & cargo '+{{ rust_nightly_external_types }}' check-external-types --manifest-path $libPkgs[$p]
        if ($LASTEXITCODE -ne 0) { $failed = $true }
    }
    if ($failed) { exit 1 }

# cargo-aprz queries the GitHub advisory API. Unauthenticated access is
# capped at 60 requests/hour and fails on a full run; an authenticated
# token raises the cap to 5000/hour. CI injects GITHUB_TOKEN
# (github.token). For local runs, if GITHUB_TOKEN is unset we borrow the
# gh CLI's stored token (non-interactive: `gh auth token` prints the
# active account's token for github.com and never opens a browser/auth
# prompt). If neither is available we warn with instructions and proceed
# unauthenticated.
#
# Unscoped (consults external risk DB).
[script("pwsh")]
anvil-aprz: anvil-aprz-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if (-not $env:GITHUB_TOKEN) {
        $tok = $null
        if (Get-Command gh -ErrorAction SilentlyContinue) {
            try { $tok = (gh auth token --hostname github.com 2>$null) } catch { $tok = $null }
        }
        if ($tok) {
            $env:GITHUB_TOKEN = $tok.Trim()
        } else {
            Write-Warning 'anvil-aprz: GITHUB_TOKEN is not set and no token could be obtained from the gh CLI.'
            Write-Warning 'cargo-aprz will use the unauthenticated GitHub API (60 requests/hour) and may fail on a full run.'
            Write-Warning 'To fix: run `gh auth login` (recommended), or set $env:GITHUB_TOKEN to a GitHub token, then re-run.'
        }
    }
    cargo aprz deps --error-if-high-risk --console appraisal
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# === pr-test members ====================================================

# Affected tier. Runs on the pinned nightly toolchain: cargo-llvm-cov
# only sets the `cfg(coverage_nightly)` that gates the workspace's
# `#[cfg_attr(coverage_nightly, coverage(off))]` exclusions when it
# instruments on nightly. On stable those exclusions are inert and
# untestable code (process-shelling error paths, script-only crates,
# etc.) wrongly counts against coverage. This matches the repo's own
# `coverage-measure` recipe, which also runs on nightly.
[script("pwsh")]
anvil-llvm-cov: anvil-llvm-cov-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-llvm-cov: no affected packages; skipping'
        exit 0
    }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Drop coverage-opted-out packages from the measured set. A crate that
    # is entirely `#![cfg_attr(coverage_nightly, coverage(off))]` (e.g. a
    # script-only crate) compiles to a test binary with no __llvm_covmap,
    # and `llvm-cov export` aborts with "no coverage data found" whenever
    # such a package is the *only* object in the report (e.g. it is the
    # sole affected crate). Opt-out is declared via
    # [package.metadata.coverage-gate] min-lines-percent = 0 -- the same
    # marker cargo-coverage-gate already treats as NO DATA -> OK. If the
    # affected set is *entirely* opted-out, there is nothing to measure,
    # so we skip exactly like the empty-affected ('--skip') case.
    if ($pkg -contains '--package') {
        $optedOut = @(
            (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
                Where-Object { $null -ne $_.metadata.'coverage-gate'.'min-lines-percent' -and [double]$_.metadata.'coverage-gate'.'min-lines-percent' -eq 0 } |
                ForEach-Object { $_.name }
        )
        if ($optedOut.Count -gt 0) {
            $filtered = [System.Collections.Generic.List[string]]::new()
            for ($i = 0; $i -lt $pkg.Count; $i++) {
                # Impact emits version-qualified specs (name@version);
                # compare the bare name against the opt-out set, but keep
                # the full name@version token in $pkg so the llvm-cov
                # measurement below resolves unambiguously.
                if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count -and $optedOut -contains (($pkg[$i + 1] -split '@', 2)[0])) {
                    $i++
                    continue
                }
                $filtered.Add($pkg[$i])
            }
            $pkg = $filtered.ToArray()
        }
        if (-not ($pkg -contains '--package')) {
            Write-Host 'anvil-llvm-cov: only coverage-opted-out packages affected; nothing to measure, skipping'
            exit 0
        }
    }
    # cargo-llvm-cov doesn't work on aarch64-pc-windows-msvc: the
    # llvm-profdata that ships with the rust toolchain there fails
    # to merge the .profraw set ("no profile can be merged"). Fall
    # back to plain `cargo nextest run` on that target so we still
    # get test execution; coverage data from this leg wouldn't have
    # been used anyway (coverage upload is gated on Linux only).
    if ($IsWindows -and ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64')) {
        Write-Host 'anvil-llvm-cov: aarch64-pc-windows-msvc -- skipping coverage; running plain nextest'
        & cargo '+{{ rust_nightly }}' nextest run @pkg --all-features --locked
        exit $LASTEXITCODE
    }
    # The *report* output directory (target/coverage/) is something we
    # choose and must exist before --output-path runs.
    [System.IO.Directory]::CreateDirectory('target/coverage') | Out-Null
    # Instrument and run tests once per feature configuration, each as a
    # SELF-CONTAINED llvm-cov run that exports only its own config's
    # objects. Dual config (--all-features + --no-default-features) catches
    # lines that only execute under one feature configuration (feature-gated
    # code dead with default features off, plus default-only code paths).
    #
    # Crucially this does NOT merge both configs' objects into one report.
    # A single `llvm-cov report` over the union shells one `--object=` per
    # test binary and, on a large workspace, overflows the Windows
    # CreateProcess command-line limit (os error 206, "filename or extension
    # is too long"). Each per-config report references only ~half the
    # objects and stays under the limit. The two configs are reconciled
    # *downstream* instead of via a merged report:
    #   * cargo-coverage-gate merges the two lcov files at the line level
    #     (per-line counts summed) -- this is the gate;
    #   * Codecov ingests both lcov files (it coalesces uploads);
    #   * ADO's PublishCodeCoverageResults coalesces both cobertura files.
    # This also avoids needing a platform-specific lcov merger (`lcov -a`
    # is Linux-only). HTML output is dropped -- it had no cloud-workflow
    # consumer and would reintroduce the overflowing union report; run
    # `cargo llvm-cov --html` ad hoc for a local viewer.
    #
    # A full `llvm-cov clean` between configs wipes the prior config's
    # objects so each `report` is scoped to the current config only.
    #
    # Note: nextest's exit-4 ("no tests to run") IS treated as a failure
    # here -- a llvm-cov run that finds no tests almost always means a
    # config mistake (wrong package filter, missing test target, etc.), not
    # a legitimate empty set. anvil-miri is the exception (see its comment).
    $configs = @(
        @{ name = 'all-features'; flag = '--all-features' },
        @{ name = 'no-default';   flag = '--no-default-features' }
    )
    foreach ($cfg in $configs) {
        cargo '+{{ rust_nightly }}' llvm-cov clean --workspace
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        & cargo '+{{ rust_nightly }}' llvm-cov nextest @pkg $cfg.flag --locked --no-report
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        # lcov feeds the gate + Codecov; cobertura feeds ADO's
        # PublishCodeCoverageResults@2. Both reports are scoped to THIS
        # config's objects, so neither overflows the Windows cmdline limit.
        cargo '+{{ rust_nightly }}' llvm-cov report --lcov      --output-path "target/coverage/lcov-$($cfg.name).info"
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        cargo '+{{ rust_nightly }}' llvm-cov report --cobertura --output-path "target/coverage/cobertura-$($cfg.name).xml"
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
    # Local coverage gate: cargo-coverage-gate compares per-package line
    # coverage against per-package thresholds carried in Cargo.toml
    # ([package.metadata.coverage-gate] min-lines-percent, falling back to
    # [workspace.metadata.coverage-gate], then the built-in 100.0 default).
    # It merges the two per-config lcovs (line-level) so the gate sees the
    # union coverage. Exits non-zero when any gated package falls below its
    # threshold, so coverage regressions fail the PR locally and in cloud
    # workflows -- the Codecov upload stays purely for display, not the gate.
    #
    # Scope the gate to the SAME package set we measured. When impact
    # scoping is active, $pkg holds `--package X@ver ...` (already opt-out
    # filtered above); pass those through so the gate only gates packages
    # that were actually instrumented -- otherwise unmeasured members would
    # report NO DATA and fail the gate. When unscoped, $pkg is `--workspace`
    # (no gate flag); drop it so the gate defaults to every member.
    #
    # cargo-coverage-gate keys on bare package names from cargo metadata, so
    # strip the @version qualifier the measurement used for disambiguation.
    $gateArgs = [System.Collections.Generic.List[string]]::new()
    for ($i = 0; $i -lt $pkg.Count; $i++) {
        if ($pkg[$i] -eq '--workspace') { continue }
        if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
            $gateArgs.Add('--package')
            $gateArgs.Add(($pkg[$i + 1] -split '@', 2)[0])
            $i++
            continue
        }
        $gateArgs.Add($pkg[$i])
    }
    cargo coverage-gate @gateArgs --lcov target/coverage/lcov-all-features.info --lcov target/coverage/lcov-no-default.info
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Affected tier. Two cargo-test invocations cover both feature
# configurations: --all-features catches doctests that only compile
# with optional features enabled; the default-features run catches
# doctests that only compile when those features are off. The
# duplication is cheap because doctests are already a small fraction
# of total compile/test time.
[script("pwsh")]
anvil-doc-test: anvil-doc-test-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    & cargo test --doc @pkg --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    & cargo test --doc @pkg --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Affected tier.
[script("pwsh")]
anvil-examples: anvil-examples-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    & cargo build @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) --examples --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# === pr-mutants member ====================================================

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

# Affected tier. cargo-mutants does its own diff-scoping via --in-diff;
# the affected-tier guard is the wiring-layer's coarse filter (if no
# affected packages exist, the entire mutants run is pointless).
#
# Skip on aarch64-pc-windows-msvc: cargo-mutants doesn't build there
# (upstream winapi incompatibility), so `_anvil-require cargo-mutants`
# would fail. The merged pr-slow group runs on all four OS legs; mutants
# is the only sub-recipe that can't follow, so it bails out early on the
# affected leg. Coverage on the other three legs is unchanged.
[script("pwsh")]
anvil-mutants-diff: anvil-mutants-diff-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($IsWindows -and ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64')) {
        Write-Host 'anvil-mutants-diff: anvil-mutants-diff-validate-prereqs aarch64-pc-windows-msvc -- cargo-mutants does not build here (winapi); skipping'
        exit 0
    }
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-mutants-diff: anvil-mutants-diff-validate-prereqs no affected packages; skipping'
        exit 0
    }
    # Resolve the diff base via the shared resolver (single source of truth
    # across the impact steps and this recipe; see _anvil-base-ref).
    $base = (& "{{just_executable()}}" _anvil-base-ref)
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    # cargo-mutants --in-diff takes a FILE path containing a unified
    # diff, not a git revision range. Write the diff to a temp file
    # first. RUNNER_TEMP (GH) and AGENT_TEMPDIRECTORY (ADO) point at
    # the job's scratch dir; fall back to the system temp dir locally.
    $tmp_dir = $env:RUNNER_TEMP
    if (-not $tmp_dir) { $tmp_dir = $env:AGENT_TEMPDIRECTORY }
    if (-not $tmp_dir) { $tmp_dir = [System.IO.Path]::GetTempPath() }
    $diff_path = Join-Path $tmp_dir 'anvil-mutants-diff.diff'
    git diff "$base..HEAD" --output=$diff_path
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    cargo mutants --in-diff $diff_path --no-shuffle --jobs 0
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# === nightly-runtime members ============================================

# Nightly checks always run full-workspace; impact env vars aren't set
# by the scheduled workflow, so the affected-tier default (--workspace)
# applies. Skip guards are still included for local diff-scoped runs.

[script("pwsh")]
anvil-miri: anvil-miri-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    # `--no-tests=pass`: miri-only exception. Tests that touch the
    # filesystem, spawn subprocesses, or use other miri-incompatible
    # APIs commonly carry `#[cfg_attr(miri, ignore)]` (this is the
    # canonical opt-out for build-tooling / CLI crates). A crate
    # whose test set ends up entirely-skipped under miri legitimately
    # produces zero runnable tests; nextest's default exit-4 ("no
    # tests to run") would fail the recipe in that case. We treat
    # empty test runs as success for miri only. Other nextest-using
    # recipes (llvm-cov) keep exit-4 as a failure because zero tests
    # there almost always indicates a config mistake.
    & cargo '+{{ rust_nightly }}' miri nextest run --no-tests=pass @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Stricter miri profiles. Each runs miri over the full workspace
# with a profile-specific MIRIFLAGS plus a profile-specific
# `--cfg miri_<profile>` set via RUSTFLAGS. Per-test opt-outs use
# `#[cfg_attr(miri_<profile>, ignore = "<reason>")]`, keeping the
# suppression next to the test rather than in a sidecar file.
# The cfg names are declared in the workspace check-cfg list
# (regions/cargo-lints-body.toml) so non-miri builds don't warn.
#
# These profiles always run full-workspace (scheduled-runtime-analysis
# does not set ANVIL_INCLUDE_AFFECTED); the --skip / affected guards
# below only apply if an adopter invokes them locally with the env
# vars set.
[script("pwsh")]
anvil-miri-tree-borrows: anvil-miri-tree-borrows-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $env:MIRIFLAGS = "-Zmiri-tree-borrows $($env:MIRIFLAGS)".Trim()
    $env:RUSTFLAGS = "--cfg miri_tree_borrows $($env:RUSTFLAGS)".Trim()
    & cargo '+{{ rust_nightly }}' miri nextest run --no-tests=pass @pkg
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[script("pwsh")]
anvil-miri-strict-provenance: anvil-miri-strict-provenance-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $env:MIRIFLAGS = "-Zmiri-strict-provenance $($env:MIRIFLAGS)".Trim()
    $env:RUSTFLAGS = "--cfg miri_strict_provenance $($env:RUSTFLAGS)".Trim()
    & cargo '+{{ rust_nightly }}' miri nextest run --no-tests=pass @pkg
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Race coverage rotates the miri-many-seeds window daily by
# day-of-month: day N -> seeds [2N-1, 2N+1) (2 seeds/day,
# ~62 seeds/month). Rotating amortizes the seed space across the
# schedule rather than retesting the same seeds every night; race
# conditions surface as inter-seed nondeterminism rather than
# per-seed crashes, so coverage matters more than depth-per-seed.
[script("pwsh")]
anvil-miri-race-coverage: anvil-miri-race-coverage-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $day = (Get-Date).Day
    $low = (2 * $day) - 1
    $high = (2 * $day) + 1
    $env:MIRIFLAGS = "-Zmiri-many-seeds=$low..$high $($env:MIRIFLAGS)".Trim()
    $env:RUSTFLAGS = "--cfg miri_race_coverage $($env:RUSTFLAGS)".Trim()
    Write-Host "anvil-miri-race-coverage: seed window $low..$high (day $day)"
    & cargo '+{{ rust_nightly }}' miri nextest run --no-tests=pass @pkg
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

[script("pwsh")]
anvil-careful: anvil-careful-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    & cargo '+{{ rust_nightly }}' careful test @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) --all-features --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Affected tier. `loom` is a permutation-based concurrency model
# checker: a crate exercises it through a dedicated `[[test]]` target
# that declares `required-features = ["loom"]` (e.g. tests/loom.rs).
# We detect those targets *structurally* from `cargo metadata` -- a
# test target whose `kind` contains "test" and whose `required-features`
# contains "loom" -- so detection is authoritative manifest data, not a
# filename / cfg / comment heuristic. Running only those targets (via
# `--test <name>`) keeps loom off the crate's ordinary tests, which must
# not run under loom.
#
# Two independent switches must both be on, and they answer different
# questions:
#   * The `loom` feature answers "which target": `required-features`
#     makes Cargo select the loom test only when the feature is enabled,
#     so a normal `cargo test` / coverage run never compiles or runs it.
#     We pass --all-features so feature-gated loom sub-modules (e.g.
#     `#![cfg(all(loom, feature = "..."))]`) still compile.
#   * `RUSTFLAGS=--cfg loom` answers "how it compiles": loom's own
#     machinery and the ecosystem gate on `cfg(loom)`, not on a feature
#     (source swaps std<->loom atomics on `#[cfg(loom)]`, and
#     `[target.'cfg(loom)'.dependencies] loom` links only under the cfg).
#     The feature does NOT replace the cfg; both are required.
# We scope `cargo test` to the owning package with `-p` (never
# `--workspace`) so a global `--cfg loom` never leaks into deps reachable
# only through *other* members (e.g. concurrent-queue), and run
# single-threaded (loom drives its own scheduling). --release per the
# loom docs: permutation exploration is wall-clock dominated by the SUT.
#
# Fail-loud guard: a crate that *declares* loom support -- a `loom`
# feature or a `cfg(loom)`-gated dependency -- but exposes no
# `required-features = ["loom"]` test target is almost certainly a
# regression (renamed/removed target), so we error rather than silently
# skip it. When no crate ships a loom target (the common case) the recipe
# skips with exit 0 -- the regular tests already ran under anvil-llvm-cov.
#
# `cargo metadata --no-deps` scans only the adopter's own members
# (layout-agnostic; works for any `members` glob). RUSTFLAGS goes via
# $env: rather than --config so it propagates to cargo's child rustc
# invocations cleanly on both pwsh/bash.
[script("pwsh")]
anvil-loom: anvil-loom-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-loom: no affected packages; skipping'
        exit 0
    }
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    # Optional impact-scoping: restrict to the affected package set.
    $affected = $null
    if ($env:ANVIL_INCLUDE_AFFECTED) {
        $affected = @((-split $env:ANVIL_INCLUDE_AFFECTED) -replace '^-p$', '' | Where-Object { $_ })
    }
    $pkgs = $meta.packages
    if ($null -ne $affected) {
        $pkgs = @($pkgs | Where-Object { $_.name -in $affected })
    }

    # Authoritative detection: [[test]] targets with required-features = ["loom"].
    $loomTargets = @(foreach ($pkg in $pkgs) {
        foreach ($t in $pkg.targets) {
            if (($t.kind -contains 'test') -and ($t.'required-features' -contains 'loom')) {
                [pscustomobject]@{ pkg = $pkg.name; target = $t.name }
            }
        }
    })
    $loomTargetPkgs = @($loomTargets | ForEach-Object { $_.pkg } | Select-Object -Unique)

    # Fail loud: crates that declare loom support but ship no loom target.
    $declarers = @(foreach ($pkg in $pkgs) {
        $hasFeature = ($pkg.features.PSObject.Properties.Name -contains 'loom')
        $hasCfgDep = [bool]($pkg.dependencies | Where-Object { $_.target -and $_.target -match 'cfg\(.*\bloom\b.*\)' })
        if ($hasFeature -or $hasCfgDep) { $pkg.name }
    })
    $missing = @($declarers | Where-Object { $_ -notin $loomTargetPkgs } | Select-Object -Unique)
    if ($missing) {
        Write-Error ('anvil-loom: crate(s) declare loom support (a loom feature or a cfg(loom) dependency) but expose no [[test]] target with required-features = ["loom"]: ' + ($missing -join ', ') + '. Add a [[test]] target with required-features = ["loom"] (e.g. tests/loom.rs), or remove the loom feature / cfg(loom) dependency.')
        exit 1
    }

    if (-not $loomTargets) {
        Write-Host 'anvil-loom: no loom test targets; skipping'
        exit 0
    }

    $env:RUSTFLAGS = "--cfg loom $($env:RUSTFLAGS)".Trim()
    foreach ($lt in $loomTargets) {
        Write-Host "anvil-loom: $($lt.pkg) :: $($lt.target)"
        & cargo test -p $lt.pkg --release --all-features --locked --test $lt.target -- --test-threads=1
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

# Affected tier. Smoke-only fuzz pass via [bolero](https://crates.io/crates/bolero):
# each fuzz target gets a 60-second libfuzzer run, just enough to
# surface obvious crashes/hangs introduced by a PR without paying for
# a long fuzzing campaign.
#
# Linux-only: cargo-bolero's libfuzzer engine (and its bolero-afl
# build dependency) only builds on Linux, so the recipe self-skips on
# non-Linux hosts -- the pr-runtime-analysis Windows/ARM legs treat
# bolero as a no-op rather than a failure. This matches oxidizer's
# reference workflow, whose bolero job runs exclusively on ubuntu.
# Crates that ship bolero harnesses still get them compiled and
# exercised as ordinary tests by anvil-llvm-cov on every leg; the
# libfuzzer campaign is the Linux-only extra.
#
# Target discovery is `cargo bolero list` over the affected packages.
# Crates with no bolero harness print zero targets and the recipe
# no-ops. The libfuzzer engine requires nightly, so the test
# invocation uses the pinned rust_nightly. Adopters who want longer
# campaigns wire a separate scheduled job; anvil deliberately does
# not ship a long-form fuzz tier (the failure surface is too
# repo-specific to standardize).
[script("pwsh")]
anvil-bolero: anvil-bolero-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if (-not $IsLinux) {
        Write-Host 'anvil-bolero: non-Linux host -- skipping (cargo-bolero/libfuzzer is Linux-only; harnesses still run under anvil-llvm-cov)'
        exit 0
    }
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') {
        Write-Host 'anvil-bolero: no affected packages; skipping'
        exit 0
    }
    $pkg = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    # Impact emits version-qualified specs (name@version); strip the
    # @version qualifier -- cargo-bolero resolves over workspace members
    # only, so bare names suffice and avoid any spec-parsing surprises.
    $bareArgs = [System.Collections.Generic.List[string]]::new()
    for ($i = 0; $i -lt $pkg.Count; $i++) {
        if ($pkg[$i] -eq '--package' -and ($i + 1) -lt $pkg.Count) {
            $bareArgs.Add('--package')
            $bareArgs.Add(($pkg[$i + 1] -split '@', 2)[0])
            $i++
            continue
        }
        $bareArgs.Add($pkg[$i])
    }
    # `cargo bolero list` emits one JSON object per target on stdout;
    # we want the set of (package, test) pairs over the affected set.
    $targetsJson = & cargo bolero list @bareArgs 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host 'anvil-bolero: bolero list failed; assuming no targets'
        exit 0
    }
    $targets = @($targetsJson | Where-Object { $_ -match '^\{' } | ForEach-Object { ConvertFrom-Json $_ })
    if ($targets.Count -eq 0) {
        Write-Host 'anvil-bolero: no fuzz targets in affected packages; skipping'
        exit 0
    }
    $failed = $false
    foreach ($t in $targets) {
        Write-Host "anvil-bolero: $($t.package_name)::$($t.test_name)"
        & cargo '+{{ rust_nightly }}' bolero test --release --engine libfuzzer -T 60s --package $t.package_name $t.test_name
        if ($LASTEXITCODE -ne 0) { $failed = $true }
    }
    if ($failed) { exit 1 }

# === nightly-exhaustive members =========================================

# Unscoped. Scheduled-exhaustive deliberately runs over the whole
# workspace regardless of diff.
[script("pwsh")]
anvil-mutants-full: anvil-mutants-full-validate-prereqs
    $ErrorActionPreference = 'Stop'
    & cargo mutants --workspace --no-shuffle --jobs 0
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Required tier. cargo-hack's feature powerset cascades through dep
# features, so the required set (workspace-internal transitive deps)
# is the right scope.
[script("pwsh")]
anvil-cargo-hack: anvil-cargo-hack-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_REQUIRED -eq '--skip') { exit 0 }
    & cargo hack @(if ($env:ANVIL_INCLUDE_REQUIRED) { -split $env:ANVIL_INCLUDE_REQUIRED } else { '--workspace' }) --feature-powerset --depth 2 check
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Affected tier.
[script("pwsh")]
anvil-bench: anvil-bench-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    & cargo bench @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' }) --all-features --no-run
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# ============================================================================
# Per-check setup + validate-prereqs
# ============================================================================
#
# Each check has matching `*-setup` and `*-validate-prereqs` recipes
# that install / verify the tools and components it needs. The setup
# recipes accept an `installer=install|install` parameter that
# forwards to the underlying tool-install recipes; the validate-prereqs
# recipes take no parameters.
#
# These are the building blocks for `anvil-<group>-setup`
# (groups.just) and `anvil-<tier>-setup` (tiers.just) ΓÇö each
# group/tier-level recipe is just a fan-out over the per-check
# setup/validate-prereqs of its members.

# --- pr-fast members ---

[group("anvil-setup")]
anvil-fmt-setup installer="install": anvil-component-nightly-rustfmt-install

[group("anvil-setup")]
anvil-fmt-validate-prereqs: anvil-component-nightly-rustfmt-validate-prereqs

[group("anvil-setup")]
anvil-clippy-setup installer="install": anvil-component-default-clippy-install

[group("anvil-setup")]
anvil-clippy-validate-prereqs: anvil-component-default-clippy-validate-prereqs

[group("anvil-setup")]
anvil-cargo-sort-setup installer="install": (anvil-tool-cargo-sort-install installer)

[group("anvil-setup")]
anvil-cargo-sort-validate-prereqs: anvil-tool-cargo-sort-validate-prereqs

[group("anvil-setup")]
anvil-license-headers-setup installer="install": (anvil-tool-cargo-heather-install installer)

[group("anvil-setup")]
anvil-license-headers-validate-prereqs: anvil-tool-cargo-heather-validate-prereqs

[group("anvil-setup")]
anvil-ensure-no-cyclic-deps-setup installer="install": (anvil-tool-cargo-ensure-no-cyclic-deps-install installer)

[group("anvil-setup")]
anvil-ensure-no-cyclic-deps-validate-prereqs: anvil-tool-cargo-ensure-no-cyclic-deps-validate-prereqs

[group("anvil-setup")]
anvil-ensure-no-default-features-setup installer="install": (anvil-tool-cargo-ensure-no-default-features-install installer)

[group("anvil-setup")]
anvil-ensure-no-default-features-validate-prereqs: anvil-tool-cargo-ensure-no-default-features-validate-prereqs

# doc-build, examples and doc-test are pure cargo built-ins; the rust
# toolchain (rustc + cargo) is the only prerequisite, and we already
# rely on it being present everywhere anvil runs.
[group("anvil-setup")]
anvil-doc-build-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-doc-build-validate-prereqs: anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-readme-check-setup installer="install": (anvil-tool-cargo-doc2readme-install installer)

[group("anvil-setup")]
anvil-readme-check-validate-prereqs: anvil-tool-cargo-doc2readme-validate-prereqs

# cargo-spellcheck has a build-time libclang dependency; the system
# deps check runs first so adopters get a clear hint instead of a
# cryptic clang-sys build error mid-install.
[group("anvil-setup")]
anvil-spellcheck-setup installer="install": anvil-system-deps-check (anvil-tool-cargo-spellcheck-install installer)

[group("anvil-setup")]
anvil-spellcheck-validate-prereqs: anvil-tool-cargo-spellcheck-validate-prereqs

# pr-title is a pwsh script; no cargo tool to install.
[group("anvil-setup")]
anvil-pr-title-setup installer="install": anvil-tool-pwsh-validate-prereqs

[group("anvil-setup")]
anvil-pr-title-validate-prereqs: anvil-tool-pwsh-validate-prereqs

[group("anvil-setup")]
anvil-deny-setup installer="install": (anvil-tool-cargo-deny-install installer)

[group("anvil-setup")]
anvil-deny-validate-prereqs: anvil-tool-cargo-deny-validate-prereqs

[group("anvil-setup")]
anvil-audit-setup installer="install": (anvil-tool-cargo-audit-install installer)

[group("anvil-setup")]
anvil-audit-validate-prereqs: anvil-tool-cargo-audit-validate-prereqs

[group("anvil-setup")]
anvil-udeps-setup installer="install": anvil-toolchain-nightly-install (anvil-tool-cargo-udeps-install installer)

[group("anvil-setup")]
anvil-udeps-validate-prereqs: anvil-toolchain-nightly-validate-prereqs anvil-tool-cargo-udeps-validate-prereqs

[group("anvil-setup")]
anvil-semver-check-setup installer="install": (anvil-tool-cargo-semver-checks-install installer)

[group("anvil-setup")]
anvil-semver-check-validate-prereqs: anvil-tool-cargo-semver-checks-validate-prereqs

[group("anvil-setup")]
anvil-external-types-setup installer="install": anvil-toolchain-nightly-external-types-install (anvil-tool-cargo-check-external-types-install installer)

[group("anvil-setup")]
anvil-external-types-validate-prereqs: anvil-toolchain-nightly-external-types-validate-prereqs anvil-tool-cargo-check-external-types-validate-prereqs

[group("anvil-setup")]
anvil-aprz-setup installer="install": (anvil-tool-cargo-aprz-install installer)

[group("anvil-setup")]
anvil-aprz-validate-prereqs: anvil-tool-cargo-aprz-validate-prereqs

# --- pr-test members (shared with scheduled-test) ---

[group("anvil-setup")]
anvil-llvm-cov-setup installer="install": anvil-component-nightly-llvm-tools-install (anvil-tool-cargo-llvm-cov-install installer) (anvil-tool-cargo-nextest-install installer) (anvil-tool-cargo-coverage-gate-install installer)

[group("anvil-setup")]
anvil-llvm-cov-validate-prereqs: anvil-component-nightly-llvm-tools-validate-prereqs anvil-tool-cargo-llvm-cov-validate-prereqs anvil-tool-cargo-nextest-validate-prereqs anvil-tool-cargo-coverage-gate-validate-prereqs

[group("anvil-setup")]
anvil-doc-test-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-doc-test-validate-prereqs: anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-examples-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-examples-validate-prereqs: anvil-tool-rustc-validate-prereqs

# --- pr-runtime-analysis members ---

[group("anvil-setup")]
anvil-miri-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install (anvil-tool-cargo-nextest-install installer)

[group("anvil-setup")]
anvil-miri-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs anvil-tool-cargo-nextest-validate-prereqs

# The three nightly-miri profiles share the same toolchain prereqs
# as anvil-miri; they only differ in the MIRIFLAGS / RUSTFLAGS the
# recipe sets. Separate setup recipes keep the per-check fan-out
# regular (groups.just can iterate without special-casing).
[group("anvil-setup")]
anvil-miri-tree-borrows-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install (anvil-tool-cargo-nextest-install installer)

[group("anvil-setup")]
anvil-miri-tree-borrows-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs anvil-tool-cargo-nextest-validate-prereqs

[group("anvil-setup")]
anvil-miri-strict-provenance-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install (anvil-tool-cargo-nextest-install installer)

[group("anvil-setup")]
anvil-miri-strict-provenance-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs anvil-tool-cargo-nextest-validate-prereqs

[group("anvil-setup")]
anvil-miri-race-coverage-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install (anvil-tool-cargo-nextest-install installer)

[group("anvil-setup")]
anvil-miri-race-coverage-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs anvil-tool-cargo-nextest-validate-prereqs

[group("anvil-setup")]
anvil-careful-setup installer="install": anvil-component-nightly-rust-src-install (anvil-tool-cargo-careful-install installer)

[group("anvil-setup")]
anvil-careful-validate-prereqs: anvil-component-nightly-rust-src-validate-prereqs anvil-tool-cargo-careful-validate-prereqs

# loom is a regular cargo-test invocation with RUSTFLAGS=--cfg loom;
# no dedicated tool to install. The rust toolchain (rustc + cargo)
# is the only prerequisite.
[group("anvil-setup")]
anvil-loom-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-loom-validate-prereqs: anvil-tool-rustc-validate-prereqs

# bolero needs cargo-bolero plus the pinned nightly toolchain
# (libfuzzer engine requires nightly rustc features).
[group("anvil-setup")]
anvil-bolero-setup installer="install": anvil-toolchain-nightly-install (anvil-tool-cargo-bolero-install installer)

[group("anvil-setup")]
anvil-bolero-validate-prereqs: anvil-toolchain-nightly-validate-prereqs anvil-tool-cargo-bolero-validate-prereqs

# --- pr-mutants members ---

[group("anvil-setup")]
anvil-mutants-diff-setup installer="install": (anvil-tool-cargo-mutants-install installer)

[group("anvil-setup")]
anvil-mutants-diff-validate-prereqs: anvil-tool-cargo-mutants-validate-prereqs

# --- scheduled-exhaustive members (mutants-full reuses cargo-mutants;
#     cargo-hack and bench are dedicated tools) ---

[group("anvil-setup")]
anvil-mutants-full-setup installer="install": (anvil-tool-cargo-mutants-install installer)

[group("anvil-setup")]
anvil-mutants-full-validate-prereqs: anvil-tool-cargo-mutants-validate-prereqs

[group("anvil-setup")]
anvil-cargo-hack-setup installer="install": (anvil-tool-cargo-hack-install installer)

[group("anvil-setup")]
anvil-cargo-hack-validate-prereqs: anvil-tool-cargo-hack-validate-prereqs

# bench uses cargo-built-ins (cargo bench --no-run + plain bench runs);
# no extra tool install needed beyond the rust toolchain.
[group("anvil-setup")]
anvil-bench-setup installer="install": anvil-tool-rustc-validate-prereqs

[group("anvil-setup")]
anvil-bench-validate-prereqs: anvil-tool-rustc-validate-prereqs