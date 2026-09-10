# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[script]
format:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    if ("{{ package }}" -ne "") {
        cargo +$env:RUST_NIGHTLY fmt -p "{{ package }}" -- --config-path ./unstable-rustfmt.toml
        return
    }

    # `cargo fmt --all` passes every source path to one rustfmt process, which
    # exceeds the Windows command-line limit in this workspace.
    $packages = (cargo metadata --format-version=1 --no-deps --locked | ConvertFrom-Json).packages.name
    foreach ($package_name in $packages) {
        cargo +$env:RUST_NIGHTLY fmt -p $package_name -- --config-path ./unstable-rustfmt.toml
    }

[script]
format-check:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    if ("{{ package }}" -ne "") {
        cargo +$env:RUST_NIGHTLY fmt -p "{{ package }}" -- --config-path ./unstable-rustfmt.toml --check
        return
    }

    $packages = (cargo metadata --format-version=1 --no-deps --locked | ConvertFrom-Json).packages.name
    foreach ($package_name in $packages) {
        cargo +$env:RUST_NIGHTLY fmt -p $package_name -- --config-path ./unstable-rustfmt.toml --check
    }
