// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(zygote_bench_wrapped)]
zygote_rt::link!();

#[path = "../../src/bin/fixture.rs"]
mod fixture;

fn main() {
    std::hint::black_box(zygote_rt::protocol::PROTOCOL_MAJOR);
    fixture::initialize_native_fixture();
    std::process::exit(fixture::run(std::env::args_os(), &[]));
}
