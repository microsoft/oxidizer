# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.
# Managed by cargo-anvil. Update the corresponding template in the cargo-anvil
# crate unless the change is repository-specific.
# Update behaviour: https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/updates.md

# See https://github.com/microsoft/ox-tools/blob/main/crates/cargo-anvil/docs/design/checks.md

# Nightly checks always run full-workspace; impact env vars aren't set
# by the scheduled workflow, so the affected-tier default (--workspace)
# applies. Skip guards are still included for local diff-scoped runs.

# Compile all selected packages together, then run their Miri test artifacts in
# parallel. The single Cargo invocation preserves workspace feature unification,
# while parallel artifact execution uses the runner cores that a normal
# `cargo miri test` invocation leaves idle.
#
# ANVIL_MIRI_JOBS overrides the automatically selected process count. The
# default uses one process per logical processor. GC memory information is
# logged as telemetry but does not limit concurrency, allowing scheduled runs
# to reveal whether memory pressure makes the CPU-based setting unsuitable for
# a runner class.

# Run selected Miri test artifacts with CPU-based parallelism.
[script("pwsh")]
_anvil-miri-test:
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }

    $packageArgs = @(if ($env:ANVIL_INCLUDE_AFFECTED) { -split $env:ANVIL_INCLUDE_AFFECTED } else { '--workspace' })
    $logRoot = Join-Path ([IO.Path]::GetTempPath()) "anvil-miri-$PID-$([guid]::NewGuid())"
    New-Item -ItemType Directory -Path $logRoot | Out-Null
    try {
        $artifactManifest = Join-Path $logRoot 'artifacts.jsonl'
        $toolchain = '+{{ rust_nightly }}'
        & cargo $toolchain miri test --all-features --tests --no-run --message-format=json-render-diagnostics @packageArgs 1> $artifactManifest
        $buildExitCode = $LASTEXITCODE

        $metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        $packageDirectories = @{}
        foreach ($package in $metadata.packages) {
            $packageDirectories[[string]$package.id] = Split-Path -Parent ([string]$package.manifest_path)
        }

        $artifacts = [System.Collections.Generic.List[object]]::new()
        $seenArtifacts = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
        foreach ($line in Get-Content -LiteralPath $artifactManifest) {
            if (-not $line.TrimStart().StartsWith('{')) {
                [Console]::Error.WriteLine($line)
                continue
            }
            $message = $line | ConvertFrom-Json -ErrorAction Stop
            if ($message.reason -eq 'compiler-message' -and $message.message.rendered) {
                [Console]::Error.Write($message.message.rendered)
            } elseif ($message.reason -eq 'compiler-artifact' -and $message.executable) {
                if ($seenArtifacts.Add([string]$message.executable)) {
                    $packageId = [string]$message.package_id
                    if (-not $packageDirectories.ContainsKey($packageId)) {
                        Write-Error "anvil miri: package metadata not found for artifact package '$packageId'"
                        exit 1
                    }
                    $name = Split-Path -Leaf ([string]$message.executable)
                    $priority = 100
                    # Start the consistently longest code-generation suites first
                    # so they do not extend the tail after short artifacts finish.
                    switch -Regex ($name) {
                        '^routerama_build-' { $priority = 0; break }
                        '^multitude_macros_impl-' { $priority = 1; break }
                        '^rest_over_grpc-' { $priority = 2; break }
                        '^internity_macros_impl-' { $priority = 3; break }
                        '^arena-' { $priority = 4; break }
                        '^enrich_err-' { $priority = 5; break }
                        '^routerama-' { $priority = 6; break }
                        '^basic-' { $priority = 7; break }
                    }
                    $artifacts.Add([pscustomobject]@{
                        Path = [string]$message.executable
                        Name = $name
                        Priority = $priority
                        WorkingDirectory = $packageDirectories[$packageId]
                    })
                }
            }
        }

        if ($buildExitCode -ne 0) { exit $buildExitCode }
        if ($artifacts.Count -eq 0) {
            Write-Error 'anvil miri: Cargo produced no runnable test artifacts'
            exit 1
        }

        $sysrootOutput = @(& cargo $toolchain miri setup --print-sysroot)
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        $env:MIRI_SYSROOT = $sysrootOutput[-1]
        $env:MIRI_BE_RUSTC = 'host'
        $runner = & rustup which --toolchain '{{ rust_nightly }}' cargo-miri
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

        $jobs = 0
        if ($env:ANVIL_MIRI_JOBS) {
            if (-not [int]::TryParse($env:ANVIL_MIRI_JOBS, [ref]$jobs) -or $jobs -lt 1) {
                Write-Error "ANVIL_MIRI_JOBS must be a positive integer, got '$($env:ANVIL_MIRI_JOBS)'"
                exit 1
            }
        } else {
            $jobs = [Environment]::ProcessorCount
        }
        $jobs = [Math]::Min($jobs, $artifacts.Count)
        [GC]::Collect()
        [GC]::WaitForPendingFinalizers()
        $memoryInfo = [GC]::GetGCMemoryInfo()
        $gcAvailableGiB = [Math]::Round($memoryInfo.TotalAvailableMemoryBytes / 1GB, 1)
        $memoryLoadGiB = [Math]::Round($memoryInfo.MemoryLoadBytes / 1GB, 1)
        $highMemoryLoadGiB = [Math]::Round($memoryInfo.HighMemoryLoadThresholdBytes / 1GB, 1)
        Write-Host "anvil miri: $([Environment]::ProcessorCount) logical processor(s); GC reports $gcAvailableGiB GiB available, $memoryLoadGiB GiB load, $highMemoryLoadGiB GiB high-load threshold"
        Write-Host "anvil miri: running $($artifacts.Count) test artifacts with $jobs concurrent process(es)"

        $orderedArtifacts = @($artifacts | Sort-Object Priority, Name)
        $work = for ($i = 0; $i -lt $orderedArtifacts.Count; $i++) {
            [pscustomobject]@{
                Index = $i
                Artifact = $orderedArtifacts[$i].Path
                Name = $orderedArtifacts[$i].Name
                WorkingDirectory = $orderedArtifacts[$i].WorkingDirectory
            }
        }
        $results = @(
            $work | ForEach-Object -Parallel {
                function Get-MemoryTelemetry {
                    [GC]::Collect()
                    [GC]::WaitForPendingFinalizers()
                    $info = [GC]::GetGCMemoryInfo()
                    $capacityGiB = [Math]::Round($info.TotalAvailableMemoryBytes / 1GB, 1)
                    $loadGiB = [Math]::Round($info.MemoryLoadBytes / 1GB, 1)
                    $headroomGiB = [Math]::Round(
                        [Math]::Max([long]0, $info.TotalAvailableMemoryBytes - $info.MemoryLoadBytes) / 1GB,
                        1
                    )
                    "GC load $loadGiB/$capacityGiB GiB, estimated headroom $headroomGiB GiB"
                }

                $item = $_
                $log = Join-Path $using:logRoot "$($item.Index).log"
                Write-Host "anvil miri: starting $($item.Name) ($(Get-MemoryTelemetry))"
                Push-Location -LiteralPath $item.WorkingDirectory
                try {
                    & $using:runner runner $item.Artifact *> $log
                    $exitCode = $LASTEXITCODE
                } finally {
                    Pop-Location
                }
                Write-Host "anvil miri: completed $($item.Name) (exit $exitCode; $(Get-MemoryTelemetry))"
                [pscustomobject]@{
                    Index = $item.Index
                    Name = $item.Name
                    ExitCode = $exitCode
                    Log = $log
                }
            } -ThrottleLimit $jobs
        )

        foreach ($result in $results | Sort-Object Index) {
            if ($env:GITHUB_ACTIONS -eq 'true') {
                Write-Host "::group::Miri artifact $($result.Name)"
            } else {
                Write-Host "`n=== Miri artifact $($result.Name) ==="
            }
            Get-Content -LiteralPath $result.Log | ForEach-Object { Write-Host $_ }
            if ($env:GITHUB_ACTIONS -eq 'true') { Write-Host '::endgroup::' }
        }

        $failed = @($results | Where-Object ExitCode -ne 0)
        if ($failed.Count -ne 0) {
            [Console]::Error.WriteLine("anvil miri: failed artifacts: $($failed.Name -join ', ')")
            exit 1
        }
    } finally {
        Remove-Item -LiteralPath $logRoot -Recurse -Force -ErrorAction SilentlyContinue
    }

# Run Miri tests for affected workspace packages.
[script("pwsh")]
anvil-miri: anvil-miri-validate-prereqs
    $ErrorActionPreference = 'Stop'
    if ($env:ANVIL_INCLUDE_AFFECTED -eq '--skip') { exit 0 }
    # Run miri via libtest (`cargo miri test`), NOT `cargo miri nextest
    # run`. nextest uses a process-per-test model; under miri every
    # process pays the full cost of re-interpreting std initialization,
    # so process-per-test roughly DOUBLES miri wall-time on a large suite
    # (the dominant cost on the PR critical path). libtest runs all tests
    # of a binary in a single process, matching how upstream runs miri.
    #
    # `--tests` selects the same target set nextest ran -- lib unit tests,
    # bin unit tests, and integration tests -- while excluding doctests
    # (miri cannot run them) and benches/examples. We use `--tests` rather
    # than `--lib --tests` because `--lib` errors with "no library targets
    # found" on a bin-only affected package under impact scoping; `--tests`
    # already covers lib unit tests and tolerates libless packages. We do
    # NOT use `--all-targets`: it would add `--benches` (criterion benches
    # have `harness = false` and run a full measurement loop -- ruinously
    # slow under miri) and `--examples` (runs each example's `main`, often
    # miri-incompatible I/O), without adding doctest coverage.
    #
    # `--all-features` exercises feature-gated code so UB hidden behind an
    # optional feature is caught, matching how upstream runs miri.
    #
    # libtest exits 0 when a binary has zero runnable tests (e.g. a crate
    # whose tests are all `#[cfg_attr(miri, ignore)]` -- the canonical
    # opt-out for build-tooling / CLI crates), so the old nextest
    # `--no-tests=pass` workaround is no longer needed.
    & "{{just_executable()}}" _anvil-miri-test
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# --- pr-runtime-analysis members ---

# Install prerequisites for the `anvil-miri` recipe.
[group("anvil-setup")]
anvil-miri-setup installer="install": anvil-component-nightly-miri-install anvil-component-nightly-rust-src-install

# Validate prerequisites for the `anvil-miri` recipe.
[group("anvil-setup")]
anvil-miri-validate-prereqs: anvil-component-nightly-miri-validate-prereqs anvil-component-nightly-rust-src-validate-prereqs
