// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod fixture;

zygote_rt::link!();

fn main() {
    fixture::initialize_native_fixture();
    std::process::exit(fixture::run(std::env::args_os(), &[]));
}
