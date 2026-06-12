// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared raw-allocation helpers used by `LocalChunk::allocate` and
//! `SharedChunk::allocate`. Both build a `header + payload_size` byte
//! allocation aligned for the chunk header, then write fields through a
//! freshly-constructed fat DST pointer.

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

/// Computes the canonical `Layout` for a chunk allocation, the single
/// source of truth shared by every `allocate`/`destroy` pair so the two
/// can never disagree (a mismatched `deallocate` layout is UB).
///
/// Two *distinct* alignments are at play and must not be conflated:
///
/// * `value_align` — the chunk type's own alignment (`align_of::<Self>()`,
///   ignoring the `[UnsafeCell<u8>]` tail which is align-1). Rust rounds
///   the size of any value up to a multiple of its alignment, so a
///   reference built from the fat pointer covers `round_up(total,
///   value_align)` bytes. The allocation's **size** is rounded up to this
///   so the reference's footprint matches the allocation exactly (a
///   shortfall is UB, caught by Miri; an excess silently over-allocates).
///
/// * `base_align` — the alignment of the allocation's **base address**,
///   which may be far larger than `value_align` (e.g. `CHUNK_ALIGN` =
///   64 KiB for shared chunks, so the chunk header is recoverable by
///   masking the low bits of any interior pointer). This governs only the
///   `Layout` alignment; the **size is never rounded up to it**, otherwise
///   every shared chunk would inflate to a full `CHUNK_ALIGN`.
///
/// `base_align >= value_align` and both must be powers of two.
#[allow(
    clippy::map_err_ignore,
    reason = "LayoutError carries no payload; only the AllocError variant matters"
)]
#[inline]
pub(crate) fn chunk_layout(header_size: usize, payload_size: usize, value_align: usize, base_align: usize) -> Result<Layout, AllocError> {
    debug_assert!(value_align.is_power_of_two(), "value_align must be a power of two");
    debug_assert!(base_align.is_power_of_two(), "base_align must be a power of two");
    debug_assert!(base_align >= value_align, "base_align must be >= value_align");
    // Round the *size* up to the value alignment (not the base alignment).
    let rounded = chunk_alloc_size(header_size, payload_size, value_align)?;
    Layout::from_size_align(rounded, base_align).map_err(|_| AllocError)
}

/// The exact byte footprint of a chunk allocation — the rounded
/// `Layout::size()` that [`chunk_layout`] produces. This is the single
/// source of truth for both the allocation `Layout` and the provider's
/// byte-budget accounting, so the two can never disagree (accounting that
/// used the unrounded `header_size + payload_size` would under-report the
/// real allocator footprint when `header_size + payload_size` is not a
/// multiple of `value_align`, e.g. for oversized chunks).
#[inline]
pub(crate) fn chunk_alloc_size(header_size: usize, payload_size: usize, value_align: usize) -> Result<usize, AllocError> {
    debug_assert!(value_align.is_power_of_two(), "value_align must be a power of two");
    let total = header_size.checked_add(payload_size).ok_or(AllocError)?;
    let mask = value_align - 1;
    Ok(total.checked_add(mask).ok_or(AllocError)? & !mask)
}

/// Allocate a `header + payload_size` byte allocation whose base address
/// is `base_align`-aligned and whose size is rounded up to `value_align`
/// (see [`chunk_layout`]).
///
/// Returns `(raw_u8_ptr, layout)` on success. The pointer carries
/// provenance over the full allocation and is suitable as the data field
/// of a slice-DST fat pointer with metadata `payload_size`. The layout is
/// the exact one passed to `allocator.allocate`, suitable for a matching
/// `deallocate` call (reproduced by [`chunk_layout`] at destroy time).
///
/// On size-overflow or end-of-address-space overflow, the allocation is
/// freed and `AllocError` is returned.
#[cfg_attr(test, mutants::skip)]
#[inline]
pub(crate) fn alloc_chunk_raw<A: Allocator>(
    allocator: &A,
    header_size: usize,
    payload_size: usize,
    value_align: usize,
    base_align: usize,
) -> Result<(*mut u8, Layout), AllocError> {
    let layout = chunk_layout(header_size, payload_size, value_align, base_align)?;
    let raw = allocator.allocate(layout)?;
    let raw_u8_ptr: *mut u8 = raw.cast::<u8>().as_ptr();
    let start_addr = raw_u8_ptr as usize;
    let end_addr = start_addr.checked_add(layout.size()).ok_or(AllocError)?;
    if end_addr > isize::MAX as usize {
        // SAFETY: matches the `allocator.allocate` pair; nothing has
        // been stored in the allocation yet.
        unsafe {
            allocator.deallocate(NonNull::new_unchecked(raw_u8_ptr), layout);
        }
        return Err(AllocError);
    }
    Ok((raw_u8_ptr, layout))
}

#[cfg(test)]
mod tests {
    use super::chunk_layout;

    /// `chunk_layout` must round the allocation *size* up to `value_align`.
    /// Pins the exact round-up so the `value_align - 1` mask can't be
    /// mutated to `value_align + 1` or `value_align / 1` — both corrupt the
    /// rounding for totals that aren't already `value_align`-aligned (the
    /// size-class tests use pre-aligned totals, so they can't catch this).
    #[test]
    fn rounds_size_up_to_value_align() {
        // A large power-of-two base (mirrors shared chunks); it governs the
        // layout *alignment* only and must not affect the size rounding.
        const BASE: usize = 65_536;
        // (header, payload, value_align, expected_size). Totals are chosen
        // to be NON-multiples of `value_align` so the mask actually rounds.
        let cases = [
            (10_usize, 7_usize, 8_usize, 24_usize), // total 17 -> 24
            (34, 16, 8, 56),                        // total 50 -> 56
            (1, 0, 8, 8),                           // total  1 -> 8
            (10, 7, 16, 32),                        // total 17 -> 32
            (5, 0, 4, 8),                           // total  5 -> 8
        ];
        for (header, payload, value_align, expected) in cases {
            let layout = chunk_layout(header, payload, value_align, BASE).expect("layout fits");
            assert_eq!(
                layout.size(),
                expected,
                "round_up({header}+{payload}, {value_align}) must be {expected}"
            );
            assert_eq!(layout.size() % value_align, 0, "size must be a multiple of value_align");
            assert_eq!(layout.align(), BASE, "alignment must be the base alignment");
        }
    }

    /// An already-`value_align`-aligned total is returned unchanged (the
    /// round-up is a no-op).
    #[test]
    fn aligned_total_is_unchanged() {
        const BASE: usize = 65_536;
        let layout = chunk_layout(8, 8, 8, BASE).expect("layout fits"); // total 16
        assert_eq!(layout.size(), 16);
    }
}
