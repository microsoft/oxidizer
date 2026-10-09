// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(zygote_bench_wrapped)]
zygote_rt::link!();

fn main() {
    std::hint::black_box(zygote_rt::protocol::PROTOCOL_MAJOR);
}
