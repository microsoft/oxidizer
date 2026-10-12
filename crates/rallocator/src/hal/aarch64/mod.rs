// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Non-faulting cache hint; null and otherwise unreadable addresses are allowed.
pub(crate) fn prefetch(address: usize) {
    // SAFETY: PRFM is an AArch64 baseline hint and does not access the pointed-to
    // memory in Rust. It may be ignored by the processor.
    unsafe {
        std::arch::asm!(
            "prfm pstl1keep, [{address}]",
            address = in(reg) address,
            options(readonly, nostack, preserves_flags),
        );
    }
}

pub(crate) fn pause() {
    std::hint::spin_loop();
}
