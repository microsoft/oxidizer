// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc types.h and page-queue.c; see LICENSE-MIMALLOC.

use std::alloc::Layout;

use crate::backend::mimalloc::hal;
use crate::backend::mimalloc::pagemap::REGION_SIZE;

pub(super) const SLICE: usize = 64 * 1024;
pub(super) const SMALL_MAX: usize = (SLICE - 4096) / 6;
pub(super) const MEDIUM_MAX: usize = (8 * SLICE - 4096) / 6;
pub(super) const LARGE_MAX: usize = 512 * 1024;
pub(super) const HUGE_BIN: usize = 73;
pub(super) const BIN_COUNT: usize = HUGE_BIN + 1;
pub(super) const DIRECT_COUNT: usize = 129;

/// Non-hardened 64-bit mimalloc bins, with two-word alignment above eight bytes.
pub(super) fn bin(size: usize) -> usize {
    let words = size.div_ceil(8);
    if words <= 8 {
        if words <= 1 { 1 } else { (words + 1) & !1 }
    } else if size > LARGE_MAX {
        HUGE_BIN
    } else {
        let words = words - 1;
        let highest = (usize::BITS - 1 - words.leading_zeros()) as usize;
        ((highest << 2) + ((words >> (highest - 2)) & 3)) - 3
    }
}

pub(super) const fn bin_size(bin: usize) -> usize {
    if bin <= 8 {
        bin * 8
    } else {
        let group = bin.div_ceil(4);
        let subdivision = (bin + 3) % 4;
        (subdivision + 5) * (1 << (group - 2)) * 8
    }
}

pub(super) fn page_size(block_size: usize) -> usize {
    if block_size <= SMALL_MAX {
        SLICE
    } else if block_size <= MEDIUM_MAX {
        8 * SLICE
    } else {
        64 * SLICE
    }
}

#[derive(Clone, Copy)]
pub(super) struct Request {
    pub(super) size: usize,
    pub(super) alignment: usize,
}

#[derive(Clone, Copy)]
enum NativeHeapState {
    Null,
    NonNull,
}

impl NativeHeapState {
    // prim-tls.h defaults to null Win32/pthread slots on Windows/macOS.
    // Linux starts with the non-null empty theap, which still permits the
    // natural-alignment shortcut in alloc-aligned.c.
    const INITIAL: Self = if cfg!(any(target_os = "windows", target_os = "macos")) {
        Self::Null
    } else {
        Self::NonNull
    };
}

impl Request {
    // Avoid a stack-returned Option<Request> in the generic allocation path.
    #[inline]
    pub(super) fn new(layout: Layout) -> Option<Self> {
        Self::aligned(layout, NativeHeapState::NonNull)
    }

    pub(super) fn for_new_thread(layout: Layout) -> Option<Self> {
        Self::aligned(layout, NativeHeapState::INITIAL)
    }

    fn aligned(layout: Layout, native_heap: NativeHeapState) -> Option<Self> {
        let size = layout.size().max(1);
        let alignment = layout.align();
        if alignment >= REGION_SIZE {
            return None;
        }
        let rounded = good_size(size)?;
        let naturally_aligned = matches!(native_heap, NativeHeapState::NonNull)
            && alignment <= size
            && ((rounded.is_power_of_two() && rounded <= 4096) || alignment == 4096 && rounded.is_multiple_of(4096));
        let size = if alignment > SLICE {
            // alloc-aligned.c avoids the small direct path for OS-aligned singletons.
            size.max(1025)
        } else if naturally_aligned {
            size
        } else {
            size.max(16).checked_add(alignment - 1)?
        };
        isize::try_from(size).ok().map(|_| Self { size, alignment })
    }

    pub(super) fn regular(size: usize) -> Option<Self> {
        isize::try_from(size)
            .ok()
            .filter(|&size| size > 0)
            .map(|_| Self { size, alignment: 8 })
    }

    pub(super) fn bin(self) -> usize {
        if self.alignment > SLICE { HUGE_BIN } else { bin(self.size) }
    }

    pub(super) fn block_size(self) -> Option<usize> {
        if self.bin() == HUGE_BIN {
            os_good_size(self.size)
        } else {
            Some(bin_size(self.bin()))
        }
    }
}

fn good_size(size: usize) -> Option<usize> {
    if size <= LARGE_MAX {
        Some(bin_size(bin(size)))
    } else {
        let alignment = hal::page_size();
        size.checked_add(alignment - 1).map(|size| size & !(alignment - 1))
    }
}

fn os_good_size(size: usize) -> Option<usize> {
    // os.c:_mi_os_good_alloc_size uses progressively coarser OS sizes.
    let alignment = if size < 512 * 1024 {
        hal::page_size()
    } else if size < 2 * 1024 * 1024 {
        64 * 1024
    } else if size < 8 * 1024 * 1024 {
        256 * 1024
    } else if size < 32 * 1024 * 1024 {
        1024 * 1024
    } else {
        4 * 1024 * 1024
    };
    size.checked_add(alignment - 1).map(|size| size & !(alignment - 1))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    macro_rules! cold_entry_case {
        ($name:ident, $size:expr, $alignment:expr, $null_size:expr, $non_null_size:expr) => {
            #[test]
            fn $name() {
                let layout = Layout::from_size_align($size, $alignment).unwrap();

                let actual = [NativeHeapState::Null, NativeHeapState::NonNull]
                    .map(|state| Request::aligned(layout, state).map(|request| (request.size, request.alignment)));

                assert_eq!(actual, [Some(($null_size, $alignment)), Some(($non_null_size, $alignment))]);
            }
        };
    }

    cold_entry_case!(cold_entry_tiny_request_keeps_native_minimum_overallocation, 1, 1, 16, 1);
    cold_entry_case!(cold_entry_word_alignment_still_selects_before_initialization, 17, 8, 24, 17);
    cold_entry_case!(cold_entry_power_of_two_keeps_alignment_slack, 64, 64, 127, 64);
    cold_entry_case!(cold_entry_class_256_uses_the_same_general_rounding_path, 256, 16, 271, 256);
    cold_entry_case!(cold_entry_non_natural_alignment_is_unchanged, 257, 256, 512, 512);
    cold_entry_case!(cold_entry_os_page_alignment_preserves_cold_slack, 4096, 4096, 8191, 4096);
    cold_entry_case!(cold_entry_larger_natural_os_alignment_preserves_cold_slack, 8192, 4096, 12287, 8192);
    cold_entry_case!(cold_entry_slice_alignment_is_not_a_singleton, 65537, 65536, 131_072, 131_072);
    cold_entry_case!(cold_entry_huge_alignment_keeps_the_small_request_floor, 128, 131_072, 1025, 1025);
    cold_entry_case!(cold_entry_huge_alignment_keeps_large_requests, 65537, 131_072, 65537, 65537);

    #[test]
    fn cold_entry_rejects_unsupported_alignment_in_both_native_states() {
        let layout = Layout::from_size_align(1, REGION_SIZE).unwrap();

        let actual = [NativeHeapState::Null, NativeHeapState::NonNull]
            .map(|state| Request::aligned(layout, state).map(|request| (request.size, request.alignment)));

        assert_eq!(actual, [None, None]);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn cold_entry_platform_uses_native_null_tls_slots() {
        let layout = Layout::from_size_align(256, 16).unwrap();

        let request = Request::for_new_thread(layout).unwrap();

        assert_eq!((request.size, request.alignment, request.block_size()), (271, 16, Some(320)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cold_entry_platform_uses_the_native_non_null_empty_heap() {
        let layout = Layout::from_size_align(256, 16).unwrap();

        let request = Request::for_new_thread(layout).unwrap();

        assert_eq!((request.size, request.alignment, request.block_size()), (256, 16, Some(256)));
    }

    #[test]
    fn bins_round_up_at_each_boundary() {
        for size in 1..=LARGE_MAX {
            let rounded = bin_size(bin(size));
            assert!(rounded >= size, "bin for {size} rounds down to {rounded}");
            assert!(rounded <= size.max(64) * 5 / 4, "bin for {size} wastes more than one subdivision");
        }
    }

    #[test]
    fn reference_bin_boundaries() {
        let requests = [1, 8, 9, 16, 17, 64, 65, 80, 81, 128, 129, 512 * 1024, 512 * 1024 + 1];
        assert_eq!(requests.map(bin), [1, 1, 2, 2, 4, 8, 9, 9, 10, 12, 13, 60, HUGE_BIN]);
    }

    #[test]
    fn bins_match_native_mimalloc_at_every_class_boundary() {
        let fixture = include_str!("../../../tests/fixtures/mimalloc-round-size.csv");
        for line in fixture.lines().filter(|line| !line.starts_with('#')) {
            let (size, rounded) = line.split_once(',').unwrap();
            let size = size.parse::<usize>().unwrap();
            let rounded = rounded.parse::<usize>().unwrap();

            assert_eq!(bin_size(bin(size)), rounded, "native mi_good_size({size})");
        }
    }

    #[test]
    fn aligned_fallback_matches_native_natural_alignment_and_overallocation() {
        let layouts = [(1, 1), (1, 8), (17, 8), (80, 8), (65, 16), (4096, 16), (65536, 8), (65536, 4096)];

        let actual = layouts.map(|(size, alignment)| {
            let request = Request::new(Layout::from_size_align(size, alignment).unwrap()).unwrap();
            (request.size, request.block_size().unwrap())
        });

        assert_eq!(
            actual,
            [
                (1, 8),
                (23, 32),
                (17, 32),
                (87, 96),
                (80, 80),
                (4096, 4096),
                (65543, 81920),
                (65536, 65536)
            ]
        );
    }

    #[test]
    fn os_alignment_uses_singletons_outside_the_small_direct_table() {
        let request = Request::new(Layout::from_size_align(1, 131_072).unwrap()).unwrap();

        assert_eq!((request.size, request.bin()), (1025, HUGE_BIN));
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn aligned_size_classes_match_the_pinned_native_allocator() {
        let fixture = include_str!("../../../tests/fixtures/mimalloc-aligned-size.windows.csv");
        for line in fixture.lines().filter(|line| !line.starts_with('#')) {
            let values = line.split(',').map(|value| value.parse::<usize>().unwrap()).collect::<Vec<_>>();
            let [size, alignment, expected] = values[..] else {
                panic!("invalid native fixture row")
            };
            let layout = Layout::from_size_align(size, alignment).unwrap();
            let request = Request::new(layout).unwrap();

            assert_eq!(
                request.block_size(),
                Some(expected),
                "native request size={size}, alignment={alignment}"
            );
        }
    }

    #[test]
    fn singleton_sizes_follow_each_os_rounding_boundary() {
        let sizes = [
            512 * 1024 + 1,
            2 * 1024 * 1024 + 1,
            8 * 1024 * 1024 + 1,
            32 * 1024 * 1024 + 1,
            usize::MAX,
        ];

        assert_eq!(
            sizes.map(os_good_size),
            [
                Some(576 * 1024),
                Some(2304 * 1024),
                Some(9 * 1024 * 1024),
                Some(36 * 1024 * 1024),
                None
            ]
        );
    }

    #[test]
    fn regular_resize_requests_reject_zero_and_overflow() {
        assert_eq!([0, usize::MAX].map(|size| Request::regular(size).is_none()), [true, true]);
    }
}
