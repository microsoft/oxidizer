// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Non-faulting cache hint; null and otherwise unreadable addresses are allowed.
#[cfg(target_os = "windows")]
// #[gamma::skip(fn_value.unit, parameter.default_shadow, reason = "PREFETCHW is explicitly non-faulting and changes only cache residency, not allocator results or ownership")]
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

/// Non-faulting hint supported by baseline Linux and macOS x86-64.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn prefetch(address: usize) {
    // SAFETY: PREFETCHT0 is an x86-64 baseline, non-faulting hint. Unlike the
    // Windows hardware contract, these platforms do not require PREFETCHW.
    unsafe {
        std::arch::asm!(
            "prefetcht0 [{address}]",
            address = in(reg) address,
            options(readonly, nostack, preserves_flags),
        );
    }
}

// #[gamma::skip(fn_value.unit, reason = "The processor spin hint changes scheduling efficiency, not the synchronization protocol")]
pub(crate) fn pause() {
    // #[gamma::skip(stmt.delete_call, reason = "spin_loop is a processor hint and does not provide synchronization or change allocator state")]
    std::hint::spin_loop();
}
