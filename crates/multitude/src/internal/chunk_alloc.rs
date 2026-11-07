// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared raw-allocation helpers for chunk `allocate` / `destroy` paths.
//! They centralize layout size and alignment.

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use crate::AllocError;

/// Computes the canonical `Layout` for a chunk allocation.
///
/// Two alignments are distinct:
///
/// * `value_align` — the chunk type's own alignment (`align_of::<Self>()`,
///   ignoring the align-1 tail). The allocation size is rounded up to this.
///
/// * `base_align` — the alignment of the allocation's **base address**,
///   which may be much larger for chunks. This governs only
///   `Layout::align`; the size is not rounded up to it.
///
/// `base_align >= value_align` and both must be powers of two.
#[expect(
    clippy::map_err_ignore,
    reason = "LayoutError carries no payload; only the AllocError variant matters"
)]
#[inline]
pub(in crate::internal) fn chunk_layout(
    header_size: usize,
    payload_size: usize,
    value_align: usize,
    base_align: usize,
) -> Result<Layout, AllocError> {
    debug_assert!(value_align.is_power_of_two(), "value_align must be a power of two");
    debug_assert!(base_align.is_power_of_two(), "base_align must be a power of two");
    debug_assert!(base_align >= value_align, "base_align must be >= value_align");
    // Round the *size* up to the value alignment (not the base alignment).
    let rounded = chunk_alloc_size(header_size, payload_size, value_align)?;
    Layout::from_size_align(rounded, base_align).map_err(|_| AllocError::CAPACITY_OVERFLOW)
}

/// Exact byte footprint of a chunk allocation: the rounded `Layout::size()`
/// produced by [`chunk_layout`]. Used for both allocation and byte-budget
/// accounting.
#[inline]
pub(in crate::internal) fn chunk_alloc_size(header_size: usize, payload_size: usize, value_align: usize) -> Result<usize, AllocError> {
    debug_assert!(value_align.is_power_of_two(), "value_align must be a power of two");
    let total = header_size.checked_add(payload_size).ok_or(AllocError::CAPACITY_OVERFLOW)?;
    let mask = value_align - 1;
    Ok(total.checked_add(mask).ok_or(AllocError::CAPACITY_OVERFLOW)? & !mask)
}

/// Allocates a chunk backing allocation using [`chunk_layout`].
///
/// Returns `(raw_u8_ptr, layout)`. The pointer covers the full allocation and
/// can be used as the data field of a slice-DST fat pointer.
///
/// On size-overflow or end-of-address-space overflow, the allocation is
/// freed and `AllocError` is returned.
#[cfg_attr(test, mutants::skip)]
#[inline]
pub(in crate::internal) fn alloc_chunk_raw<A: Allocator>(
    allocator: &A,
    header_size: usize,
    payload_size: usize,
    value_align: usize,
    base_align: usize,
) -> Result<(*mut u8, Layout), AllocError> {
    alloc_chunk_raw_with_address(allocator, header_size, payload_size, value_align, base_align, |pointer| {
        pointer as usize
    })
}

pub(in crate::internal) fn alloc_chunk_raw_with_address<A: Allocator>(
    allocator: &A,
    header_size: usize,
    payload_size: usize,
    value_align: usize,
    base_align: usize,
    address: impl FnOnce(*mut u8) -> usize,
) -> Result<(*mut u8, Layout), AllocError> {
    let layout = chunk_layout(header_size, payload_size, value_align, base_align)?;
    let raw_u8_ptr = checked_allocation_with(
        layout,
        |layout| Ok(allocator.allocate(layout)?.cast::<u8>().as_ptr()),
        address,
        |pointer, layout| {
            // SAFETY: Rejection returns the unchanged exclusively owned allocation and layout.
            unsafe { allocator.deallocate(NonNull::new_unchecked(pointer), layout) };
        },
    )?;
    Ok((raw_u8_ptr, layout))
}

fn checked_allocation_with<T: Copy>(
    layout: Layout,
    allocate: impl FnOnce(Layout) -> Result<T, AllocError>,
    address: impl FnOnce(T) -> usize,
    release: impl FnOnce(T, Layout),
) -> Result<T, AllocError> {
    let raw = allocate(layout)?;
    let start_addr = address(raw);
    validate_allocation_end_with(start_addr, layout.size(), || {
        release(raw, layout);
    })?;
    Ok(raw)
}

fn validate_allocation_end_with(start_addr: usize, size: usize, release: impl FnOnce()) -> Result<(), AllocError> {
    let end_addr = start_addr.checked_add(size).ok_or(AllocError::CAPACITY_OVERFLOW)?;
    if end_addr > isize::MAX as usize {
        release();
        return Err(AllocError::CAPACITY_OVERFLOW);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{checked_allocation_with, chunk_layout, validate_allocation_end_with};
    use crate::AllocError;

    #[test]
    fn gamma_nov4_checked_allocation_preserves_exact_address_layout_and_release_order() {
        let layout = core::alloc::Layout::from_size_align(8, 8).unwrap();
        for (start, accepted, released) in [(isize::MAX as usize - 8, true, false), (isize::MAX as usize - 7, false, true)] {
            let events = core::cell::RefCell::new(alloc::vec::Vec::new());
            let result = checked_allocation_with(
                layout,
                |actual| {
                    assert_eq!(actual, layout);
                    events.borrow_mut().push(("allocate", 71, actual.size()));
                    Ok(71usize)
                },
                |token| {
                    assert_eq!(token, 71);
                    events.borrow_mut().push(("address", token, start));
                    start
                },
                |token, actual| {
                    assert_eq!(actual, layout);
                    events.borrow_mut().push(("release", token, actual.size()));
                },
            );
            assert_eq!(result, if accepted { Ok(71) } else { Err(AllocError::CAPACITY_OVERFLOW) });
            let mut expected = alloc::vec![("allocate", 71, 8), ("address", 71, start)];
            if released {
                expected.push(("release", 71, 8));
            }
            assert_eq!(*events.borrow(), expected);
        }
        assert_eq!(
            checked_allocation_with(
                layout,
                |_| Err::<usize, _>(AllocError::CAPACITY_OVERFLOW),
                |_| panic!("failed allocation has no address"),
                |_, _| panic!("failed allocation cannot release"),
            )
            .unwrap_err(),
            AllocError::CAPACITY_OVERFLOW
        );
    }

    #[test]
    fn gamma_round2_address_validation_releases_only_the_rejected_high_range() {
        for (start, size, accepted, expected_releases) in [
            (64, 8, true, 0),
            (isize::MAX as usize - 8, 8, true, 0),
            (isize::MAX as usize - 8, 9, false, 1),
            (usize::MAX, 1, false, 0),
        ] {
            let mut releases = 0;
            let result = validate_allocation_end_with(start, size, || releases += 1);
            assert_eq!(result.is_ok(), accepted, "start={start}, size={size}");
            assert_eq!(releases, expected_releases, "start={start}, size={size}");
        }
    }

    /// `chunk_layout` must round allocation size up to `value_align`.
    #[test]
    fn rounds_size_up_to_value_align() {
        // Large base alignment must not affect size rounding.
        const BASE: usize = 65_536;
        // Non-multiple totals force the rounding mask to matter.
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

#[cfg(test)]
mod gamma_survivor_tests {
    use super::*;

    #[test]
    fn gamma_survivor_chunk_layout_independent_integer_oracle() {
        for align in [1_usize, 2, 4, 8, 16, 64, 4096] {
            for header in [0_usize, 1, 7, 16, 33, usize::MAX - 7, usize::MAX] {
                for payload in [0_usize, 1, 7, 64, 511, usize::MAX] {
                    let wide = (header as u128) + (payload as u128);
                    let rounded = wide.div_ceil(align as u128) * (align as u128);
                    let actual = chunk_alloc_size(header, payload, align);
                    if let Ok(expected) = usize::try_from(rounded) {
                        assert_eq!(actual.unwrap(), expected);
                        let layout = chunk_layout(header, payload, align, 65536);
                        let expected_layout = Layout::from_size_align(expected, 65536);
                        match expected_layout {
                            Ok(expected_layout) => assert_eq!(layout.unwrap(), expected_layout),
                            Err(_) => gamma_survivor_expect_error(layout),
                        }
                    } else {
                        gamma_survivor_expect_error(actual);
                        gamma_survivor_expect_error(chunk_layout(header, payload, align, 65536));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn gamma_survivor_expect_error<T, E>(result: Result<T, E>) {
    let Err(_) = result else {
        panic!("the fixture requires this operation to return its injected error");
    };
    drop(result);
}
