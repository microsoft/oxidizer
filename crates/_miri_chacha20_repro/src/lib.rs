// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Throwaway reproducer crate to verify the precise failure mode of
//! `rand` 0.10's default `ThreadRng` (backed by `chacha20`'s NEON
//! backend) under Miri on `aarch64-unknown-linux-gnu`.
//!
//! This crate intentionally has no production code; the single test
//! below is the minimum surface area required to trigger the
//! `llvm.aarch64.neon.tbl1.v16i8` unsupported-operation abort in
//! Miri so the reproducer can be cited in upstream bug reports.

#[cfg(test)]
mod tests {
    #[test]
    fn rand_random_u32_triggers_chacha20_generate() {
        // A single draw is enough: ThreadRng's BlockRng will call
        // chacha20::ChaChaCore::generate, which dispatches to the
        // NEON backend on aarch64 and ends up invoking vqtbl1q_u8.
        let _: u32 = rand::random();
    }
}
