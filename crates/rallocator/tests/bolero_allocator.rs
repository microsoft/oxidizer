// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded allocation traces shared by property tests and coverage-guided fuzzing.

#![cfg(not(miri))]
#![expect(clippy::unwrap_used, reason = "Test invariants fail immediately with the offending input")]

use bolero::TypeGenerator;

mod common;
mod scenarios;

// SAFETY: This is the only rallocator configuration used in this test binary.
static ALLOCATOR: rallocator::Rallocator = unsafe { rallocator::Rallocator::new() };

#[test]
fn allocation_sequences() {
    // The generator's own length must reach a full trace: `with_max_len` bounds
    // the driver's byte budget, while `Vec` generation otherwise stops at its
    // default 64-element cap and would decode only a few operations. The budget
    // exceeds the trace so the length discriminator does not starve the final
    // instruction's bytes.
    bolero::check!()
        .with_generator(Vec::<u8>::produce().with().len(0..=scenarios::INPUT_BYTES))
        .with_max_len(scenarios::INPUT_BYTES + 16)
        .for_each(|input: &Vec<u8>| scenarios::run(&ALLOCATOR, input));
}
