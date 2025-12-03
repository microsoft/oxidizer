# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

# https://users.rust-lang.org/t/how-to-document-optional-features-in-api-docs/64577

$env:RUSTDOCFLAGS="--cfg docsrs"
cargo +nightly doc --all-features --no-deps --open
