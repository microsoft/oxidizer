// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Non-faulting cache hint; null and otherwise unreadable addresses are allowed.
#[cfg(target_os = "windows")]
pub(crate) fn prefetch(address: usize) {
    // SAFETY: PrefetchW is part of Windows 10's x64 hardware baseline and is a
    // non-faulting hint; the address need not be readable or dereferenceable.
    // This matches the pinned native MSVC PrefetchForWrite instruction.
    unsafe {
        std::arch::asm!(
            "prefetchw [{address}]",
            address = in(reg) address,
            options(readonly, nostack, preserves_flags),
        );
    }
}

/// Non-faulting hint supported by the baseline Linux x86-64 instruction set.
#[cfg(target_os = "linux")]
pub(crate) fn prefetch(address: usize) {
    // SAFETY: PREFETCHT0 is an x86-64 baseline, non-faulting hint. Unlike the
    // Windows hardware contract, Linux does not require PREFETCHW support.
    unsafe {
        std::arch::asm!(
            "prefetcht0 [{address}]",
            address = in(reg) address,
            options(readonly, nostack, preserves_flags),
        );
    }
}

pub(crate) fn pause() {
    std::hint::spin_loop();
}
