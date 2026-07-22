# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# Run Loom model-checked tests.
#
# Loom is a permutation tester: it explores every legal interleaving of the
# atomic operations the C++/Rust memory model allows. It catches memory-
# ordering bugs that hardware-stressing won't reproduce reliably.
#
# Discovery is convention-based: any package whose `Cargo.toml` declares a
# `[target.'cfg(loom)'.dependencies]` section is included automatically.
# No workflow or recipe edits are required when a new package opts in.
#
# Tuning:
#   LOOM_MAX_PREEMPTIONS=2   (default) bound on context switches Loom tries.
#                            Smaller = faster, less coverage.
#   LOOM_LOG=info            print per-iteration thread schedules.

# Run Loom model-checked tests for every package that opts in via cfg(loom).
[script]
loom FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Build a manifest-path → package-name map so we use the real Cargo
    # package name for `-p` instead of the directory name (they can differ).
    $metadata = cargo metadata --format-version=1 --no-deps --locked | ConvertFrom-Json
    $packagesByManifest = @{}
    foreach ($pkg in $metadata.packages) {
        $norm = [System.IO.Path]::GetFullPath($pkg.manifest_path)
        $packagesByManifest[$norm] = $pkg.name
    }

    $packages = @()
    foreach ($dir in (Get-ChildItem crates -Directory -ErrorAction SilentlyContinue)) {
        $manifest = Join-Path $dir.FullName "Cargo.toml"
        if ((Test-Path $manifest) -and (Select-String -Path $manifest -Pattern "cfg\(loom\)" -Quiet)) {
            $norm = [System.IO.Path]::GetFullPath($manifest)
            $pkgName = $packagesByManifest[$norm]
            if (-not $pkgName) {
                throw "Could not determine cargo package name for $manifest"
            }
            $packages += [pscustomobject]@{ Name = $pkgName; Dir = $dir.FullName }
        }
    }

    if ($packages.Count -eq 0) {
        Write-Host "No packages declare loom tests (looking for [target.'cfg(loom)'.dependencies])."
        return
    }

    Write-Host "Loom-enabled packages: $($packages.Name -join ', ')" -ForegroundColor Cyan

    # `--release` makes Loom's exploration substantially faster: the
    # instrumented atomic primitives are themselves expensive in debug mode.
    # We restrict execution to `tests/loom_*.rs` integration test files —
    # the standard convention. Inline `#[test]`s in `src/` use real atomics
    # and would panic under `--cfg loom` if they were swept in here.
    $original_rustflags = $env:RUSTFLAGS
    $env:RUSTFLAGS = if ($original_rustflags) { "$original_rustflags --cfg loom" } else { "--cfg loom" }
    try {
        foreach ($package in $packages) {
            $tests_dir = Join-Path $package.Dir "tests"
            $loom_files = @()
            if (Test-Path $tests_dir) {
                $loom_files = Get-ChildItem -Path $tests_dir -Filter "loom_*.rs" -File -ErrorAction SilentlyContinue
            }
            if ($loom_files.Count -eq 0) {
                Write-Host "`n=== $($package.Name) has cfg(loom) deps but no tests/loom_*.rs files; skipping ===" -ForegroundColor Yellow
                continue
            }
            foreach ($f in $loom_files) {
                $name = [System.IO.Path]::GetFileNameWithoutExtension($f.Name)
                Write-Host "`n=== Loom: $($package.Name)::$name ===" -ForegroundColor Cyan
                cargo +$env:RUST_LATEST test --release -p $package.Name --test $name --features loom --locked -- --test-threads=1 {{ FILTER }}
            }
        }
    } finally {
        # Restore the caller's RUSTFLAGS rather than blanket-deleting it,
        # so a pre-existing value the user had set survives this recipe.
        if ($null -eq $original_rustflags) {
            Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
        } else {
            $env:RUSTFLAGS = $original_rustflags
        }
    }

# Run Bolero property-based / coverage-guided fuzz tests.
#
# Bolero drives random sequences against fuzz targets defined in
# `tests/bolero_*.rs` files. Each `#[test]` invoking `bolero::check!()`
# is a target; targets are auto-discovered via `cargo bolero list`.
#
# Discovery is convention-based: any package with `tests/bolero_*.rs`
# files contributes targets. No workflow or recipe edits are required
# when a new package opts in.
#
# The `libfuzzer` engine is used (Linux only). On other platforms,
# the targets still run as plain `#[test]`s under `cargo test`.
#
# TIME bounds the per-target fuzz duration (default 60s). Override
# locally for longer runs, e.g. `just bolero 10m`.

# Run Bolero fuzz tests for every package with tests/bolero_*.rs files.
[script]
bolero TIME="60s":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Build a manifest-path → package-name map so we use the real Cargo
    # package name for `-p` instead of the directory name (they can differ).
    $metadata = cargo metadata --format-version=1 --no-deps --locked | ConvertFrom-Json
    $packagesByManifest = @{}
    foreach ($pkg in $metadata.packages) {
        $norm = [System.IO.Path]::GetFullPath($pkg.manifest_path)
        $packagesByManifest[$norm] = $pkg.name
    }

    # Discover packages with bolero integration tests.
    $entries = @()
    foreach ($dir in (Get-ChildItem crates -Directory -ErrorAction SilentlyContinue)) {
        $tests_dir = Join-Path $dir.FullName "tests"
        if (-not (Test-Path $tests_dir)) { continue }
        $files = Get-ChildItem -Path $tests_dir -Filter "bolero_*.rs" -File -ErrorAction SilentlyContinue
        if (-not $files) { continue }

        $manifest = [System.IO.Path]::GetFullPath((Join-Path $dir.FullName "Cargo.toml"))
        $pkgName = $packagesByManifest[$manifest]
        if (-not $pkgName) {
            throw "Could not determine cargo package name for $manifest"
        }

        foreach ($f in $files) {
            $entries += [pscustomobject]@{
                Package = $pkgName
                Harness = [System.IO.Path]::GetFileNameWithoutExtension($f.Name)
            }
        }
    }

    if ($entries.Count -eq 0) {
        Write-Host "No packages declare bolero tests (looking for tests/bolero_*.rs)."
        return
    }

    Write-Host "Bolero harnesses:" -ForegroundColor Cyan
    foreach ($e in $entries) { Write-Host "  $($e.Package)::$($e.Harness)" }

    # libfuzzer requires nightly. On non-Linux hosts the engine is unavailable,
    # so we fall back to running the tests as ordinary `cargo test` invocations
    # (each `#[test]` decorated with `bolero::check!()` is a regular test
    # there, exercising the deterministic in-tree corpus only).
    $is_linux = $IsLinux -or ($PSVersionTable.PSEdition -eq 'Core' -and -not $IsWindows -and -not $IsMacOS)
    if (-not $is_linux) {
        Write-Host "`nNon-Linux host: running bolero harnesses as plain tests (libfuzzer unavailable)." -ForegroundColor Yellow
        foreach ($e in $entries) {
            Write-Host "`n=== bolero (plain): $($e.Package)::$($e.Harness) ===" -ForegroundColor Cyan
            cargo +$env:RUST_NIGHTLY test -p $e.Package --test $e.Harness --all-features --locked
        }
        return
    }

    foreach ($e in $entries) {
        Write-Host "`n=== bolero list: $($e.Package)::$($e.Harness) ===" -ForegroundColor Cyan
        $listing = cargo +$env:RUST_NIGHTLY bolero list -p $e.Package
        if (-not $listing) {
            Write-Host "  (no targets)"
            continue
        }

        # `cargo bolero list` emits one JSON object per line.
        foreach ($line in ($listing -split "`n")) {
            $line = $line.Trim()
            if (-not $line) { continue }
            $entry = $line | ConvertFrom-Json
            $target = $entry.test
            if (-not $target) { $target = $entry.test_name }
            if (-not $target) { $target = $entry.target }
            Write-Host "`n=== bolero test: $($e.Package)::$($e.Harness)::$target (time={{ TIME }}) ===" -ForegroundColor Cyan
            # Raise the RSS limit from the 2 GiB default. Adaptive chunk sizing
            # allocates more (smaller, 64 KiB-aligned) chunks per arena, which
            # inflates ASan shadow memory and quarantine overhead under libfuzzer.
            cargo +$env:RUST_NIGHTLY bolero test $target -p $e.Package --engine libfuzzer -T {{ TIME }} --engine-args=-rss_limit_mb=4096
        }
    }
