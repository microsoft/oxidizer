// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Non-hardened sizeclassconfig.h / sizeclasstable.h configuration.

use std::alloc::Layout;

pub(crate) const CHUNK_BITS: usize = 14;
pub(crate) const CHUNK: usize = 1 << CHUNK_BITS;
pub(crate) const SMALL_MAX: usize = 1 << 16;
pub(crate) const COUNT: usize = 44;
pub(crate) const SMALL_TAG: usize = 64;

#[derive(Clone, Copy)]
pub(crate) struct Class {
    pub(crate) size: usize,
    pub(crate) slab: usize,
    pub(crate) capacity: u16,
    pub(crate) waking: u16,
}

const fn make_classes() -> [Class; COUNT] {
    let mut classes = [Class {
        size: 0,
        slab: 0,
        capacity: 0,
        waking: 0,
    }; COUNT];
    let mut i = 0;
    while i < COUNT {
        let size = if i < 4 {
            (i + 1) * 16
        } else {
            let exponent = (i - 4) / 4;
            let mantissa = (i - 4) % 4;
            (5 + mantissa) << (4 + exponent)
        };
        let slab = if size * 4 < CHUNK { CHUNK } else { (size * 4).next_power_of_two() };
        #[expect(clippy::cast_possible_truncation, reason = "These size classes have at most 1024 objects per slab")]
        let capacity = (slab / size) as u16;
        classes[i] = Class {
            size,
            slab,
            capacity,
            waking: if capacity / 4 < 32 { capacity / 4 } else { 32 },
        };
        i += 1;
    }
    classes
}

pub(crate) const CLASSES: [Class; COUNT] = make_classes();

const fn make_lookup() -> [u8; SMALL_MAX / 16] {
    let mut table = [0; SMALL_MAX / 16];
    let mut bucket = 0;
    let mut class = 0;
    while bucket < table.len() {
        while CLASSES[class].size < bucket * 16 + 1 {
            class += 1;
        }
        #[expect(clippy::cast_possible_truncation, reason = "The lookup contains only the 44 small-class indices")]
        let index = class as u8;
        table[bucket] = index;
        bucket += 1;
    }
    table
}

// sizeclasstable.h's 4096-byte SizeClassLookup: one entry per 16-byte step.
static SIZE_LOOKUP: [u8; SMALL_MAX / 16] = make_lookup();

#[inline]
pub(crate) fn index(size: usize) -> usize {
    usize::from(SIZE_LOOKUP[(size.max(1) - 1) >> 4])
}

/// A validated class, constructed only from a valid `Layout`. Keeping the tag
/// private makes a mismatched (tag, size) impossible at the core boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    tag: u8,
}

impl Request {
    #[inline]
    pub(crate) fn new(layout: Layout) -> Option<Self> {
        Self::with_size(layout, layout.size())
    }

    /// Reuses a valid layout's alignment while validating a replacement size.
    /// Class rounding already rejects every result exceeding `isize::MAX`, so
    /// constructing another `Layout` would duplicate size and alignment checks.
    #[inline]
    pub(crate) fn with_size(layout: Layout, size: usize) -> Option<Self> {
        let alignment = layout.align();
        let padded = size.max(1).checked_add(alignment - 1)? & !(alignment - 1);
        if padded <= SMALL_MAX {
            // Class steps are powers of two, preserving padded alignment.
            #[expect(clippy::cast_possible_truncation, reason = "64 plus a small-class index is at most 107")]
            let tag = (SMALL_TAG + index(padded)) as u8;
            return Some(Self { tag });
        }
        let size = padded.checked_next_power_of_two()?;
        let size = isize::try_from(size).ok()?;
        #[expect(clippy::cast_possible_truncation, reason = "A 64-bit leading-zero count plus one fits in a byte")]
        let tag = (size.leading_zeros() + 1) as u8;
        Some(Self { tag })
    }

    pub(crate) fn tag(self) -> usize {
        usize::from(self.tag)
    }

    pub(crate) fn size(self) -> usize {
        size(self.tag())
    }
}

pub(crate) fn size(tag: usize) -> usize {
    if tag >= SMALL_TAG {
        CLASSES[tag - SMALL_TAG].size
    } else {
        1usize << (64 - tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_table_generation_matches_compiled_tables() {
        for (generated, compiled) in make_classes().iter().zip(CLASSES) {
            assert_eq!(
                (generated.size, generated.slab, generated.capacity, generated.waking),
                (compiled.size, compiled.slab, compiled.capacity, compiled.waking)
            );
        }
        assert_eq!(make_lookup(), SIZE_LOOKUP);
    }

    #[test]
    fn all_class_boundaries() {
        assert_eq!(CLASSES[COUNT - 1].size, SMALL_MAX);
        for n in 1..=SMALL_MAX {
            let i = index(n);
            assert!(CLASSES[i].size >= n);
            assert!(i == 0 || CLASSES[i - 1].size < n);
        }
    }

    #[test]
    fn class_rounding_preserves_alignment() {
        for shift in 0..=20 {
            let align = 1usize << shift;
            for n in 1..=65537 {
                let request = Request::new(Layout::from_size_align(n, align).unwrap()).unwrap();
                let actual = request.size();
                assert!(actual >= n);
                assert_eq!(actual % align, 0);
                assert_eq!(size(request.tag()), actual);
            }
        }
    }

    #[test]
    fn validated_request_preserves_zero_large_and_overflow_cases() {
        for (size, expected) in [
            (0, 16),
            (65536, 65536),
            (65537, 131_072),
            (1 << 46, 1 << 46),
            ((1 << 46) + 1, 1 << 47),
        ] {
            let request = Request::new(Layout::from_size_align(size, 1).unwrap()).unwrap();
            assert_eq!(request.size(), expected);
        }
        // Classification must not move the backend's 64TiB rejection earlier:
        // larger representable classes still enter the normal slow allocation.
        assert!(Request::new(Layout::from_size_align(1 << 62, 1).unwrap()).is_some());
        assert_eq!(Request::new(Layout::from_size_align(isize::MAX as usize, 1).unwrap()), None);
    }

    #[test]
    fn replacement_classification_matches_checked_layout_construction() {
        for alignment in [1, 16, 64, 4096, 1 << 46, 1 << 62, 1 << 63] {
            let layout = Layout::from_size_align(0, alignment).unwrap();
            for size in [
                0,
                1,
                16,
                17,
                65535,
                65536,
                65537,
                1 << 46,
                (1 << 46) + 1,
                1 << 62,
                (1 << 62) + 1,
                isize::MAX as usize,
                usize::MAX,
            ] {
                let expected = Layout::from_size_align(size, alignment).ok().and_then(Request::new);
                assert_eq!(Request::with_size(layout, size), expected, "{size}/{alignment}");
            }
        }
    }

    #[test]
    fn classification_matches_explicit_alignment_padding() {
        for alignment in [1, 2, 4, 8, 16, 32, 64, 4096, 1 << 46, 1 << 62, 1 << 63] {
            let layout = Layout::from_size_align(0, alignment).unwrap();
            let boundaries =
                (0..=SMALL_MAX * 2 + 1).chain([1 << 46, (1 << 46) + 1, 1 << 62, (1 << 62) + 1, isize::MAX as usize, usize::MAX]);
            for size in boundaries {
                let expected = size
                    .max(1)
                    .checked_add(alignment - 1)
                    .map(|sum| sum & !(alignment - 1))
                    .and_then(|padded| {
                        if padded <= SMALL_MAX {
                            Some(CLASSES[index(padded)].size)
                        } else {
                            padded
                                .checked_next_power_of_two()
                                .filter(|&rounded| isize::try_from(rounded).is_ok())
                        }
                    });
                assert_eq!(Request::with_size(layout, size).map(Request::size), expected, "{size}/{alignment}");
            }
        }
    }
}
