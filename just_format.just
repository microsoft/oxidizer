# fmt needs a different way to specify the package.
format_package := if package == "" { " --all" } else { " -p " + package }

[group('format')]
[script]
format:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo +$env:RUST_NIGHTLY fmt {{ format_package }} -- --config-path ./unstable-rustfmt.toml

[group('format')]
[script]
format-check:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo +$env:RUST_NIGHTLY fmt {{ format_package }} --check
