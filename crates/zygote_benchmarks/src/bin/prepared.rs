// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod fixture;

use zygote_rt::{Launch, Prepared, ZygoteSafe};

struct State {
    bytes: Vec<u8>,
}

// SAFETY: the fixture state is immutable owned bytes with no threads, handles,
// synchronization, secrets, or pointers into storage outside the allocation.
unsafe impl ZygoteSafe for State {}

#[expect(clippy::cast_sign_loss, reason = "index modulo 251 always fits in u8")]
fn prepare() -> Result<Prepared<State>, &'static str> {
    fixture::initialize_native_fixture();
    let byte_count = match env!("CARGO_BIN_NAME") {
        "zygote_bench_prepared_0" => 0,
        "zygote_bench_prepared_1m" => 1 << 20,
        "zygote_bench_prepared_64m" => 64 << 20,
        _ => return Err("unexpected prepared benchmark binary name"),
    };
    let bytes = (0..byte_count).map(|index| (index % 251) as u8).collect();
    Ok(Prepared::new(State { bytes }))
}

fn application(state: &'static State, launch: Launch<'_>) -> i32 {
    fixture::run(launch.into_args_os(), &state.bytes)
}

zygote_rt::prepared_main!(prepare, application);
