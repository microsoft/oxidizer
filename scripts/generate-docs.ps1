# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

<#
.SYNOPSIS
    Generates documentation for all crates with all features enabled.

.DESCRIPTION
    This script generates documentation locally using the nightly toolchain
    with the docsrs configuration flag. This allows viewing documentation
    for feature-gated items that would normally only appear on docs.rs.

    Reference: https://users.rust-lang.org/t/how-to-document-optional-features-in-api-docs/64577

.EXAMPLE
    .\scripts\generate-docs.ps1

    Generates documentation and opens it in the default browser.
#>

$env:RUSTDOCFLAGS="--cfg docsrs"
cargo +nightly doc --all-features --no-deps --open
