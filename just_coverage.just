# llvm-cov tool needs a different way to specify the package.
coverage-package := if package == "" { "" } else { "-p " + package }

[script]
coverage-measure:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Before running the tests, we need to clear old test coverage data because the coverage report
    # simply sums up all the data in the target folder, even if it is from old builds.
    cargo +$env:RUST_NIGHTLY llvm-cov clean --workspace

    # This will run tests and generate test coverage data files, to be analyzed separately by coverage-report.
    cargo +$env:RUST_NIGHTLY llvm-cov nextest {{ target_package }} --no-fail-fast --all-targets --no-report --all-features --locked

[script]
coverage-report:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo +$env:RUST_NIGHTLY llvm-cov report {{ coverage-package }} --open
