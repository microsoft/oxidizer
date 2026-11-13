// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See ../LICENSE-MIMALLOC.

pub(super) fn thread_environment_block() -> *const u8 {
    let teb;
    // SAFETY: The Windows ARM64 ABI reserves x18 for the current TEB. The
    // instruction copies its value without accessing memory or changing x18.
    unsafe {
        std::arch::asm!(
            "mov {}, x18",
            out(reg) teb,
            options(nostack, preserves_flags, nomem),
        );
    }
    teb
}
