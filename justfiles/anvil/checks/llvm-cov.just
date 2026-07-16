# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Owned by cargo-anvil; edit via `cargo anvil`.
# See ../../../docs/design/checks.md for the catalog and rationale.

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

# --- pr-test members (shared with scheduled-test) ---

[group("anvil-setup")]
anvil-llvm-cov-setup installer="install": anvil-component-nightly-llvm-tools-install (anvil-tool-cargo-llvm-cov-install installer) (anvil-tool-cargo-nextest-install installer) (anvil-tool-cargo-coverage-gate-install installer)

[group("anvil-setup")]
anvil-llvm-cov-validate-prereqs: anvil-component-nightly-llvm-tools-validate-prereqs anvil-tool-cargo-llvm-cov-validate-prereqs anvil-tool-cargo-nextest-validate-prereqs anvil-tool-cargo-coverage-gate-validate-prereqs
