// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Slot geometry: the offsets and stride of a `#[repr(C)] SlotCell<T>`,
//! expressed as a pure function of the value's size and alignment.
//!
//! Two independent consumers must agree on these numbers exactly: the pool,
//! when it lays out a chunk and addresses a slot, and the handle, when it walks
//! from a value pointer back to the slot and the chunk header. The handle
//! cannot ask the pool, because finding the pool is the point of the walk, so
//! agreement is structural — both evaluate the formulas here over the same
//! inputs. See `docs/implementation/geometry.md`.

use core::alloc::Layout;
use core::marker::PhantomData;

use crate::atomic::AtomicU32;
use crate::chunk::ChunkHeader;

/// Rounds `x` up to the next multiple of `align`, which must be a power of two.
#[inline]
#[must_use]
pub(crate) const fn round_up(x: usize, align: usize) -> usize {
    (x + align - 1) & !(align - 1)
}

/// Alignment of one slot: the value's alignment widened to hold the trailing
/// `u32` metadata words.
#[inline]
#[must_use]
pub(crate) const fn cell_align(align: usize) -> usize {
    let refcount = align_of::<AtomicU32>();
    let index = align_of::<u32>();
    let wider = if refcount > index { refcount } else { index };
    if align > wider { align } else { wider }
}

/// Byte offset of the reference count within a slot.
#[inline]
#[must_use]
pub(crate) const fn refcount_offset(size: usize) -> usize {
    round_up(size, align_of::<AtomicU32>())
}

/// Byte offset of the in-chunk index within a slot.
#[inline]
#[must_use]
pub(crate) const fn index_offset(size: usize) -> usize {
    round_up(refcount_offset(size) + size_of::<AtomicU32>(), align_of::<u32>())
}

/// Distance between consecutive slots.
#[inline]
#[must_use]
pub(crate) const fn stride(size: usize, align: usize) -> usize {
    round_up(index_offset(size) + size_of::<u32>(), cell_align(align))
}

/// Byte offset of the slot payload within a chunk allocation. Independent of
/// the chunk's slot count, so recovery is pure arithmetic.
#[inline]
#[must_use]
pub(crate) const fn slots_offset(align: usize) -> usize {
    round_up(size_of::<ChunkHeader>(), cell_align(align))
}

/// Alignment of a whole chunk allocation.
#[inline]
#[must_use]
#[cfg_attr(test, mutants::skip)] // `>` vs `>=` is equivalent here: when the two are equal both arms return the same value.
pub(crate) const fn chunk_align(align: usize) -> usize {
    let header = align_of::<ChunkHeader>();
    let cell = cell_align(align);
    if header > cell { header } else { cell }
}

/// Computes the [`Layout`] of a chunk holding `slots` slots, or `None` if the
/// arithmetic overflows.
#[inline]
#[must_use]
pub(crate) fn chunk_layout(size: usize, align: usize, slots: usize) -> Option<Layout> {
    let payload = stride(size, align).checked_mul(slots)?;
    let total = slots_offset(align).checked_add(payload)?;
    Layout::from_size_align(total, chunk_align(align)).ok().map(|l| l.pad_to_align())
}

/// Supplies the slot geometry for one pool.
///
/// The type parameter form answers with compile-time constants, so a typed pool
/// emits the same code it would with the formulas written inline. The runtime
/// form answers with loaded fields, which is what lets one pool body serve a
/// layout that is only known at run time.
pub(crate) trait SlotGeometry: Copy {
    /// Distance between consecutive slots.
    fn stride(self) -> usize;
    /// Byte offset of the reference count within a slot.
    fn refcount_offset(self) -> usize;
    /// Byte offset of the in-chunk index within a slot.
    fn index_offset(self) -> usize;
    /// Byte offset of the slot payload within a chunk allocation.
    fn slots_offset(self) -> usize;
    /// Layout of a chunk holding `slots` slots, or `None` on overflow.
    fn chunk_layout(self, slots: usize) -> Option<Layout>;
}

/// Geometry of a pool whose element type is fixed at compile time.
///
/// Zero-sized, and every accessor folds to a constant, so storing it in the
/// pool costs nothing and reading it emits no code.
pub(crate) struct TypedGeometry<T>(PhantomData<fn() -> T>);

impl<T> TypedGeometry<T> {
    /// Cross-checks the formulas against the compiler's own layout of the slot
    /// type, so every element type the crate is instantiated with re-verifies
    /// them against ground truth.
    ///
    /// Referenced from the accessors below rather than from a dedicated entry
    /// point, because an associated `const` is evaluated only where it is used
    /// and no instantiation may route around it.
    const CHECK: () = {
        let size = size_of::<T>();
        let align = align_of::<T>();
        assert!(stride(size, align) == size_of::<crate::slot::SlotCell<T>>(), "slot stride must equal the compiler's slot size");
        assert!(cell_align(align) == align_of::<crate::slot::SlotCell<T>>(), "slot alignment must equal the compiler's slot alignment");
        assert!(refcount_offset(size) == core::mem::offset_of!(crate::slot::SlotCell<T>, refcount), "refcount offset must equal the compiler's field offset");
        assert!(index_offset(size) == core::mem::offset_of!(crate::slot::SlotCell<T>, index), "index offset must equal the compiler's field offset");
    };

    /// Returns the geometry for `T`.
    #[inline]
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Clone for TypedGeometry<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for TypedGeometry<T> {}

impl<T> SlotGeometry for TypedGeometry<T> {
    #[inline]
    fn stride(self) -> usize {
        () = Self::CHECK;
        stride(size_of::<T>(), align_of::<T>())
    }

    #[inline]
    fn refcount_offset(self) -> usize {
        () = Self::CHECK;
        refcount_offset(size_of::<T>())
    }

    #[inline]
    fn index_offset(self) -> usize {
        () = Self::CHECK;
        index_offset(size_of::<T>())
    }

    #[inline]
    fn slots_offset(self) -> usize {
        () = Self::CHECK;
        slots_offset(align_of::<T>())
    }

    #[inline]
    fn chunk_layout(self, slots: usize) -> Option<Layout> {
        () = Self::CHECK;
        chunk_layout(size_of::<T>(), align_of::<T>(), slots)
    }
}

/// Geometry of a pool whose value layout is fixed at construction time.
///
/// The offsets are precomputed once and stored, so the allocation path loads
/// them rather than recomputing them.
#[derive(Clone, Copy)]
pub(crate) struct RuntimeGeometry {
    /// Size of the value stored in a slot.
    size: usize,
    /// Alignment of the value stored in a slot.
    align: usize,
    /// Distance between consecutive slots.
    stride: usize,
    /// Byte offset of the reference count within a slot.
    refcount_offset: usize,
    /// Byte offset of the in-chunk index within a slot.
    index_offset: usize,
    /// Byte offset of the slot payload within a chunk allocation.
    slots_offset: usize,
}

impl RuntimeGeometry {
    /// Builds the geometry serving `layout`.
    #[inline]
    #[must_use]
    pub(crate) fn new(layout: Layout) -> Self {
        let size = layout.size();
        let align = layout.align();
        Self {
            size,
            align,
            stride: stride(size, align),
            refcount_offset: refcount_offset(size),
            index_offset: index_offset(size),
            slots_offset: slots_offset(align),
        }
    }
}

impl SlotGeometry for RuntimeGeometry {
    #[inline]
    fn stride(self) -> usize {
        self.stride
    }

    #[inline]
    fn refcount_offset(self) -> usize {
        self.refcount_offset
    }

    #[inline]
    fn index_offset(self) -> usize {
        self.index_offset
    }

    #[inline]
    fn slots_offset(self) -> usize {
        self.slots_offset
    }

    #[inline]
    fn chunk_layout(self, slots: usize) -> Option<Layout> {
        chunk_layout(self.size, self.align, slots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot::SlotCell;

    /// Checks both providers against the compiler's own layout of `SlotCell<$t>`.
    ///
    /// The `const CHECK` already does this for every instantiated element type,
    /// but only where an accessor is called; running it as a test states the
    /// property explicitly and covers types the crate is never instantiated with.
    macro_rules! check_type {
        ($t:ty) => {{
            let size = size_of::<$t>();
            let align = align_of::<$t>();
            let typed = TypedGeometry::<$t>::new();
            let runtime = RuntimeGeometry::new(Layout::new::<$t>());

            assert_eq!(cell_align(align), align_of::<SlotCell<$t>>(), "cell alignment for {}", stringify!($t));
            assert_eq!(typed.stride(), size_of::<SlotCell<$t>>(), "stride for {}", stringify!($t));
            assert_eq!(
                typed.refcount_offset(),
                core::mem::offset_of!(SlotCell<$t>, refcount),
                "refcount offset for {}",
                stringify!($t),
            );
            assert_eq!(
                typed.index_offset(),
                core::mem::offset_of!(SlotCell<$t>, index),
                "index offset for {}",
                stringify!($t),
            );

            // The two providers are separate code paths reaching the same
            // numbers; a blind pool addressing a layout must land exactly where
            // a typed pool for that layout would.
            assert_eq!(runtime.stride(), typed.stride());
            assert_eq!(runtime.refcount_offset(), typed.refcount_offset());
            assert_eq!(runtime.index_offset(), typed.index_offset());
            assert_eq!(runtime.slots_offset(), typed.slots_offset());
            assert_eq!(runtime.chunk_layout(7), typed.chunk_layout(7));
            assert_eq!(typed.chunk_layout(7), chunk_layout(size, align, 7));
        }};
    }

    #[repr(align(64))]
    struct OverAligned {
        _pad: u8,
    }

    #[repr(align(4096))]
    struct PageAligned {
        _pad: [u8; 8192],
    }

    #[test]
    fn formulas_match_compiler_layout() {
        check_type!(());
        check_type!(u8);
        check_type!(u16);
        check_type!(u32);
        check_type!(u64);
        check_type!(u128);
        check_type!([u8; 3]);
        check_type!([u8; 7]);
        check_type!([u64; 9]);
        check_type!(OverAligned);
        check_type!(PageAligned);
        check_type!(Option<alloc::boxed::Box<u32>>);
    }

    #[test]
    fn slots_are_reachable_within_the_chunk() {
        for &(size, align) in &[(0_usize, 1_usize), (1, 1), (3, 1), (8, 8), (12, 4), (1, 64), (8192, 4096)] {
            let geometry = RuntimeGeometry::new(Layout::from_size_align(size, align).unwrap());
            let slots = 5;
            let layout = geometry.chunk_layout(slots).expect("layout fits");

            assert_eq!(layout.align(), chunk_align(align));
            assert!(layout.align() >= align_of::<ChunkHeader>());
            assert!(layout.align() >= align);
            assert!(layout.size() >= geometry.slots_offset() + slots * geometry.stride());
            assert!(geometry.slots_offset() >= size_of::<ChunkHeader>());

            // Every slot's metadata must lie inside the value's own stride, or
            // consecutive slots would overlap.
            assert!(geometry.refcount_offset() >= size);
            assert!(geometry.index_offset() >= geometry.refcount_offset() + size_of::<AtomicU32>());
            assert!(geometry.stride() >= geometry.index_offset() + size_of::<u32>());
            assert_eq!(geometry.stride() % cell_align(align), 0);
            assert_eq!(geometry.slots_offset() % cell_align(align), 0);
        }
    }

    #[test]
    fn chunk_layout_reports_overflow() {
        let geometry = RuntimeGeometry::new(Layout::new::<u64>());
        assert!(geometry.chunk_layout(usize::MAX).is_none());
        assert!(geometry.chunk_layout(usize::MAX / 4).is_none());
    }

    #[test]
    fn chunk_layout_accepts_a_zero_slot_chunk() {
        let geometry = RuntimeGeometry::new(Layout::new::<u64>());
        let layout = geometry.chunk_layout(0).expect("layout fits");
        assert!(layout.size() >= size_of::<ChunkHeader>());
    }

    #[test]
    fn round_up_leaves_aligned_values_alone() {
        assert_eq!(round_up(0, 8), 0);
        assert_eq!(round_up(8, 8), 8);
        assert_eq!(round_up(1, 8), 8);
        assert_eq!(round_up(9, 8), 16);
        assert_eq!(round_up(7, 1), 7);
    }

    #[test]
    fn typed_geometry_is_free_to_store() {
        assert_eq!(size_of::<TypedGeometry<u64>>(), 0);
        assert_eq!(size_of::<TypedGeometry<PageAligned>>(), 0);
    }
}
