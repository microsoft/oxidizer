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

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use core::alloc::Layout;
use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::atomic::AtomicU32;
use crate::chunk::ChunkHeader;
use crate::slot::SlotCell;

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
#[cfg_attr(test, mutants::skip)] // The first comparison is between two equal alignments; the second differs from `>=` only when its operands are equal. Both arms then return the same value.
pub(crate) const fn cell_align(align: usize) -> usize {
    let refcount = align_of::<AtomicU32>();
    let index = align_of::<u32>();
    let wider = if refcount > index { refcount } else { index };
    if align > wider { align } else { wider }
}

/// The key a multi pool routes on: the part of a layout that its slot geometry
/// actually depends on.
///
/// Every offset, the stride and the chunk shape derive from the size and from
/// [`cell_align`] of the alignment, so two layouts that agree on both produce
/// byte-identical chunks and are served by one pool. Widening is idempotent, so
/// building a pool from the key yields the geometry the original layout asked
/// for, and a value is never under-aligned: the key's alignment is never
/// narrower than the layout's own.
/// Ref: docs/design/multi-pool.md, "Exact sizes, no size classes".
#[inline]
#[must_use]
pub(crate) const fn routing_key(layout: Layout) -> Layout {
    let align = cell_align(layout.align());

    // Widening cannot make a representable layout unrepresentable: the floor is
    // a primitive alignment, and a layout describing a value leaves that much
    // room below the size ceiling.
    debug_assert!(
        Layout::from_size_align(layout.size(), align).is_ok(),
        "widening a value layout must stay representable"
    );

    // SAFETY: `align` is a power of two, being either the layout's own or the
    // alignment of a primitive type, and the size fits as argued above.
    unsafe { Layout::from_size_align_unchecked(layout.size(), align) }
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
///
/// # Safety
/// Each geometry value describes a single value layout. Every copy of that
/// value must report the same stride, metadata offsets, slot-array offset, and
/// chunk-layout result for the same slot count while the pool uses it.
/// Ref: docs/implementation/geometry.md, "One formula, two consumers".
///
/// When [`chunk_layout`](Self::chunk_layout) returns `Some(layout)`, an
/// allocation with that layout must be suitable for a [`ChunkHeader`] at the
/// allocation base and for consecutive slots beginning
/// [`slots_offset`](Self::slots_offset) bytes from that base, separated by
/// [`stride`](Self::stride) bytes. For every slot address produced this way:
/// the slot address must be the value address for the geometry's value layout;
/// the value address must satisfy that layout's alignment; the reported
/// reference-count and index offsets must identify in-bounds, properly aligned
/// fields for `AtomicU32` and `u32`; and the stride must cover the value and
/// metadata fields without overlap.
///
/// [`slot_at`](Self::slot_at) must return the corresponding slot address for a
/// chunk and in-chunk offset. [`header_of`](Self::header_of) must be its inverse
/// for a slot address and that slot's stored in-chunk index.
pub(crate) unsafe trait SlotGeometry: Copy {
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

    /// Returns the address of slot `offset` within the chunk headed by `chunk`.
    ///
    /// # Safety
    /// `chunk` must head a live chunk laid out by this geometry, and `offset`
    /// must be less than that chunk's slot count.
    #[inline]
    unsafe fn slot_at(self, chunk: NonNull<ChunkHeader>, offset: usize) -> NonNull<u8> {
        // SAFETY: the payload begins `slots_offset` bytes into the chunk and
        // holds at least `offset + 1` slots by the caller's contract.
        unsafe {
            let first = chunk.as_ptr().cast::<u8>().add(self.slots_offset());
            NonNull::new_unchecked(first.add(offset * self.stride()))
        }
    }

    /// Recovers the owning chunk header from a slot address and the slot's
    /// (already read) in-chunk index.
    ///
    /// # Safety
    /// `slot` must address a live slot in a chunk laid out by this geometry,
    /// and `index` must be that slot's stored in-chunk index.
    #[inline]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "the recovered header sits at a `ChunkHeader`-aligned offset by construction of the chunk layout"
    )]
    unsafe fn header_of(self, slot: NonNull<u8>, index: u32) -> NonNull<ChunkHeader> {
        // SAFETY: stepping back `index` slots lands on the first slot, and
        // stepping back `slots_offset` further lands on the chunk header.
        unsafe {
            let back = index as usize * self.stride() + self.slots_offset();
            NonNull::new_unchecked(slot.as_ptr().sub(back).cast::<ChunkHeader>())
        }
    }
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
    /// Forced from the constructor, which is the only way to obtain a value of
    /// this type, so no instantiation can route around it.
    const CHECK: () = {
        let size = size_of::<T>();
        let align = align_of::<T>();
        assert!(
            stride(size, align) == size_of::<crate::slot::SlotCell<T>>(),
            "slot stride must equal the compiler's slot size"
        );
        assert!(
            cell_align(align) == align_of::<crate::slot::SlotCell<T>>(),
            "slot alignment must equal the compiler's slot alignment"
        );
        assert!(
            refcount_offset(size) == core::mem::offset_of!(crate::slot::SlotCell<T>, refcount),
            "refcount offset must equal the compiler's field offset"
        );
        assert!(
            index_offset(size) == core::mem::offset_of!(crate::slot::SlotCell<T>, index),
            "index offset must equal the compiler's field offset"
        );
    };

    /// Returns the geometry for `T`.
    #[inline]
    #[must_use]
    pub(crate) const fn new() -> Self {
        () = Self::CHECK;
        Self(PhantomData)
    }
}

impl<T> Clone for TypedGeometry<T> {
    // Required by `Copy`, which is how the geometry is actually passed around.
    // Written out rather than derived, because `derive` would demand `T: Clone`.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for TypedGeometry<T> {}

// SAFETY: `TypedGeometry<T>` derives every number from `T`'s size and
// alignment, and `CHECK` compares the stride, alignment and metadata offsets
// with the compiler's `#[repr(C)] SlotCell<T>` layout. Chunk layout uses the
// same formulas for the header and slot array, and the typed addressing
// overrides use `SlotCell<T>` pointer arithmetic over that slot array, so the
// two addressing directions agree with the layout they describe.
// Ref: docs/implementation/geometry.md, "Proving the formulas".
unsafe impl<T> SlotGeometry for TypedGeometry<T> {
    #[inline]
    fn stride(self) -> usize {
        stride(size_of::<T>(), align_of::<T>())
    }

    #[inline]
    fn refcount_offset(self) -> usize {
        refcount_offset(size_of::<T>())
    }

    #[inline]
    fn index_offset(self) -> usize {
        index_offset(size_of::<T>())
    }

    #[inline]
    fn slots_offset(self) -> usize {
        slots_offset(align_of::<T>())
    }

    #[inline]
    fn chunk_layout(self, slots: usize) -> Option<Layout> {
        chunk_layout(size_of::<T>(), align_of::<T>(), slots)
    }

    /// Addresses the slot as the compiler lays out `[SlotCell<T>]`, rather than
    /// by multiplying out the stride. `CHECK` proves the two agree, and the
    /// typed form gives the optimizer the element type directly.
    #[inline]
    unsafe fn slot_at(self, chunk: NonNull<ChunkHeader>, offset: usize) -> NonNull<u8> {
        // SAFETY: the payload begins `slots_offset` bytes into the chunk and
        // holds at least `offset + 1` slots by the caller's contract.
        unsafe {
            let first = chunk.as_ptr().cast::<u8>().add(self.slots_offset()).cast::<SlotCell<T>>();
            NonNull::new_unchecked(first.add(offset).cast::<u8>())
        }
    }

    /// The inverse of [`slot_at`](Self::slot_at), in the same terms.
    #[inline]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "the slot payload and the recovered header both sit at their natural offsets by construction of the chunk layout"
    )]
    unsafe fn header_of(self, slot: NonNull<u8>, index: u32) -> NonNull<ChunkHeader> {
        // SAFETY: stepping back `index` slots lands on the first slot, and
        // stepping back `slots_offset` further lands on the chunk header.
        unsafe {
            let first = slot.as_ptr().cast::<SlotCell<T>>().sub(index as usize);
            NonNull::new_unchecked(first.cast::<u8>().sub(self.slots_offset()).cast::<ChunkHeader>())
        }
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

    /// The value layout this geometry serves.
    #[inline]
    #[must_use]
    pub(crate) fn layout(self) -> Layout {
        // SAFETY: the size and alignment were taken from a valid `Layout`.
        unsafe { Layout::from_size_align_unchecked(self.size, self.align) }
    }
}

// SAFETY: `RuntimeGeometry::new` captures a valid `Layout`'s size and
// alignment, precomputes every number from the shared formulas, and stores
// those immutable values in the geometry. The default addressing methods use
// the same stored stride and slot-array offset that `chunk_layout` uses when
// sizing the chunk allocation, so allocation and pointer recovery describe the
// same slots.
// Ref: docs/implementation/geometry.md, "One derivation, two shapes".
unsafe impl SlotGeometry for RuntimeGeometry {
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    /// Checks both providers against the compiler's own layout of `SlotCell<$t>`,
    /// and the formulas against `core`'s own `repr(C)` field placement.
    ///
    /// The `const CHECK` already checks the providers for every instantiated
    /// element type, but only where an accessor is called; running it as a test
    /// states the property explicitly and covers types the crate is never
    /// instantiated with.
    macro_rules! check_type {
        ($t:ty) => {{
            let size = size_of::<$t>();
            let align = align_of::<$t>();
            let typed = TypedGeometry::<$t>::new();
            let runtime = RuntimeGeometry::new(Layout::new::<$t>());

            assert_eq!(
                cell_align(align),
                align_of::<SlotCell<$t>>(),
                "cell alignment for {}",
                stringify!($t)
            );
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

            // A multi pool addressing a layout must land exactly where a typed
            // pool for that layout would.
            assert_eq!(runtime.stride(), typed.stride(), "runtime stride for {}", stringify!($t));
            assert_eq!(
                runtime.refcount_offset(),
                typed.refcount_offset(),
                "runtime refcount offset for {}",
                stringify!($t),
            );
            assert_eq!(
                runtime.index_offset(),
                typed.index_offset(),
                "runtime index offset for {}",
                stringify!($t)
            );
            assert_eq!(runtime.slots_offset(), typed.slots_offset());
            assert_eq!(runtime.chunk_layout(7), typed.chunk_layout(7));
            assert_eq!(typed.chunk_layout(7), chunk_layout(size, align, 7));

            // The formulas are hand-rolled for speed; `Layout::extend` is
            // `core`'s own `repr(C)` field-placement algorithm and reaches the
            // same numbers from an independent implementation.
            let value = Layout::new::<$t>();
            let (with_refcount, extend_refcount) = value.extend(Layout::new::<AtomicU32>()).unwrap();
            let (with_index, extend_index) = with_refcount.extend(Layout::new::<u32>()).unwrap();
            let slot = with_index.pad_to_align();
            let (_, extend_slots) = Layout::new::<ChunkHeader>().extend(slot).unwrap();

            assert_eq!(
                typed.refcount_offset(),
                extend_refcount,
                "extend refcount offset for {}",
                stringify!($t)
            );
            assert_eq!(typed.index_offset(), extend_index, "extend index offset for {}", stringify!($t));
            assert_eq!(typed.stride(), slot.size(), "extend stride for {}", stringify!($t));
            assert_eq!(typed.slots_offset(), extend_slots, "extend slots offset for {}", stringify!($t));
            assert_eq!(cell_align(align), slot.align(), "extend cell alignment for {}", stringify!($t));
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
            let layout = geometry.chunk_layout(slots).unwrap();

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
        let layout = geometry.chunk_layout(0).unwrap();
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
