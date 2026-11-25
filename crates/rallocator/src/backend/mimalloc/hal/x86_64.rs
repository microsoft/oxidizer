// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See ../LICENSE-MIMALLOC.

pub(super) fn thread_environment_block() -> *const u8 {
    let teb;
    // SAFETY: Windows x64 keeps NT_TIB.Self at GS:0x30. This reads the
    // current thread's TEB pointer without changing registers or memory.
    unsafe {
        std::arch::asm!(
            "mov {}, gs:[0x30]",
            out(reg) teb,
            options(nostack, preserves_flags, readonly),
        );
    }
    teb
}
