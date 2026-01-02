[group('basic')]
build PROFILE='dev':
    cargo build {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets --locked

# Run unit and integration tests with extra standard libbrary debug assertions enabled.
[group('basic')]
[script]
careful FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo +$env:RUST_NIGHTLY careful test {{ target_package }} --no-fail-fast --all-features --locked -- {{ FILTER }}

[group('basic')]
check PROFILE='dev':
    cargo check {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets --locked

[group('basic')]
clippy PROFILE='dev':
    cargo clippy {{ target_package }} --profile {{ PROFILE }} --all-targets --all-features --locked

[group('basic')]
deny:
    cargo deny fetch all
    cargo deny --locked check

[group('basic')]
[script]
docs:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo doc {{ target_package }} --no-deps --all-features --locked

# Run all stand-alone example binaries to ensure they complete with exit code 0.
[group('basic')]
[script]
examples PROFILE='dev':
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    . ./scripts/run-examples.ps1 -Package "{{ package }}" -Profile "{{ PROFILE }}"

[group('basic')]
hack:
    cargo hack {{ target_package }} check --feature-powerset --locked

# Run unit and integration tests under Miri.
[group('basic')]
[script]
miri FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo +$env:RUST_NIGHTLY miri nextest run {{ target_package }} --no-fail-fast --all-features --locked -- {{ FILTER }}

# Run an example binary under Miri.
[group('basic')]
[script]
miri-example EXAMPLE_NAME:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    if ("{{ package }}" -eq "") {
        cargo +$env:RUST_NIGHTLY miri run --all-features --example {{ EXAMPLE_NAME }} --locked
    } else {
        cargo +$env:RUST_NIGHTLY miri run -p {{ package }} --all-features --example {{ EXAMPLE_NAME }} --locked
    }

# Runs unit and integration tests.
[group('basic')]
test FILTER="":
    cargo nextest run {{ target_package }} --no-fail-fast --all-features --locked -- {{ FILTER }}

# Runs inline examples in documentation comments (doctests).
[group('basic')]
[script]
test-docs FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    if ("{{ package }}" -eq "") {
        cargo test --doc --no-fail-fast --all-features --locked -- {{ FILTER }}
    } else {
        # Single package - check if it's a library crate, otherwise skip it.
        $lib_rs_path = "crates/{{ package }}/src/lib.rs"

        if (Test-Path $lib_rs_path) {
            cargo test {{ target_package }} --doc --no-fail-fast --all-features --locked -- {{ FILTER }}
        } else {
            # Binary-only crate - skip doctests because Cargo expects library targets for doctests.
            # Binary crates (with only main.rs) cannot contain doctests, so running `cargo test --doc`
            # on them results in an error rather than gracefully running zero tests.
            Write-Host "Skipping doctests for binary-only package: {{ package }}"
        }
    }

# Same as 'test' but also run one iteration of benchmarks.
[group('basic')]
test-more FILTER="":
    # We separate this from "test" because the benchmark infra is quite slow, so we want to avoid
    # running them for quick iterative testing, only running them in thorough validation workflows.
    cargo nextest run {{ target_package }} --no-fail-fast --all-features --locked --tests --benches -- {{ FILTER }}

# Detect unused dependencies.
[group('basic')]
[script]
udeps:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Try both with and without feature flags to cover both extremes.
    cargo +$env:RUST_NIGHTLY udeps {{ target_package }} --locked --all-features --all-targets
    cargo +$env:RUST_NIGHTLY udeps {{ target_package }} --locked --all-targets
