// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-allocation drop record stored at the high end of a chunk's payload.

// `drop_shim` and `replay_drops` are `unsafe fn` with documented safety
// contracts at the function level; inner unsafe wrappers add no extra
// safety boundary.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use core::sync::atomic::{AtomicPtr, Ordering};
use core::{mem, ptr};

/// Drop shim signature.
///
/// `(value_ptr, count)` runs `drop_in_place::<[T]>` on `count` consecutive
/// `T`s starting at `value_ptr`. The element type `T` is baked into the
/// concrete shim instantiation, so the replay loop is type-erased.
pub(crate) type DropFn = unsafe fn(*mut u8, usize);

/// Alignment we want each in-place drop entry to sit at, so a packed sequence
/// of entries at the chunk tail stays naturally aligned.
const PAD_TARGET: usize = mem::align_of::<DropFn>();

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // `+ → *` observationally equivalent (2+2 == 2*2)
const fn raw_used() -> usize {
    mem::size_of::<DropFn>() + mem::size_of::<u16>() + mem::size_of::<u16>()
}

#[cfg_attr(coverage_nightly, coverage(off))]
const fn pad_bytes() -> usize {
    let raw = raw_used();
    if raw.is_multiple_of(PAD_TARGET) {
        0
    } else {
        PAD_TARGET - (raw % PAD_TARGET)
    }
}

const PAD_BYTES: usize = pad_bytes();

/// A single entry in a chunk's trailing drop list.
///
/// Entries grow downward from the high end of the payload; bump allocations
/// grow upward from the low end.
///
/// # Two-phase commit
///
/// Allocation writes a [`DropEntry::placeholder`] and increments the count.
/// Initialization later calls [`DropEntry::commit_drop_fn`]. Uncommitted
/// placeholders are skipped during replay.
#[repr(C)]
pub(crate) struct DropEntry {
    /// Type-erased shim. `AtomicPtr<()>` preserves function-pointer
    /// provenance across the store/load round-trip. Null means
    /// "uncommitted placeholder".
    ///
    /// The placeholder → committed transition is race-safe because
    /// concurrent `assume_init` callers install the *same* function
    /// pointer (the shim is determined by `T`), making the relaxed
    /// store idempotent.
    drop_fn: AtomicPtr<()>,

    /// Byte offset of the value (or first slice element) from the start of
    /// the chunk's payload. Capped at 64 KiB by the `u16` width.
    value_offset: u16,

    /// Number of `T` elements at `value_offset`. `1` for single values.
    len: u16,

    /// Padding so successive entries on the back-stack remain naturally
    /// aligned to `align_of::<DropFn>()`.
    _pad: [u8; PAD_BYTES],
}

impl DropEntry {
    /// Constructs a placeholder entry with no drop shim attached.
    #[inline]
    pub(crate) const fn placeholder(value_offset: u16, len: u16) -> Self {
        Self {
            drop_fn: AtomicPtr::new(ptr::null_mut()),
            value_offset,
            len,
            _pad: [0; PAD_BYTES],
        }
    }

    /// Fills in the real drop shim pointer. Idempotent under races: when
    /// two threads commit the same slot, both writes are the same value
    /// (the shim is determined by `T`), so a relaxed-store is sufficient
    /// once paired with the `Acquire` load in [`replay_drops`].
    #[inline]
    pub(crate) fn commit_drop_fn(&self, drop_fn: DropFn) {
        // Cast the fn pointer to `*mut ()` for atomic storage; this
        // preserves the function pointer's provenance, which a
        // `fn-as-usize` round-trip would lose (yielding a `noalloc`
        // pointer on load).
        #[allow(
            clippy::fn_to_numeric_cast_any,
            reason = "intentional: bit-cast a function pointer for atomic storage; provenance preserved via `*mut ()`"
        )]
        let raw = drop_fn as *mut ();
        self.drop_fn.store(raw, Ordering::Release);
    }

    /// Returns the committed drop shim, if any.
    #[inline]
    pub(crate) fn drop_fn(&self) -> Option<DropFn> {
        let raw = self.drop_fn.load(Ordering::Acquire);
        if raw.is_null() {
            None
        } else {
            // SAFETY: a non-null value was installed by a prior valid
            // `commit_drop_fn` (Release) and the matching Acquire load
            // synchronises with it; the stored pointer carries its
            // original function-pointer provenance.
            Some(unsafe { mem::transmute::<*mut (), DropFn>(raw) })
        }
    }

    /// Returns the byte offset of the value from the start of the chunk's
    /// payload.
    #[inline]
    pub(crate) fn value_offset(&self) -> u16 {
        self.value_offset
    }

    /// Returns the number of `T` elements at the value offset.
    #[inline]
    pub(crate) fn len(&self) -> u16 {
        self.len
    }
}

/// A type-erased drop shim for `count` consecutive `T`s.
///
/// `ptr` must be aligned for `T` and point at `count` initialized `T`s. This
/// function is invoked once per committed [`DropEntry`] during chunk teardown.
///
/// # Safety
///
/// Callers must guarantee the alignment and initialization preconditions
/// described above; calling this on uninitialized storage or with a mismatched
/// `T` is undefined behavior.
pub(crate) unsafe fn drop_shim<T>(ptr: *mut u8, count: usize) {
    let slice = ptr::slice_from_raw_parts_mut(ptr.cast::<T>(), count);
    // SAFETY: by the function's safety contract.
    ptr::drop_in_place(slice);
}

/// Replays committed drop entries packed against the high end of `payload`.
///
/// Entries grow downward from the payload end and are replayed newest-first
/// (LIFO), so child values drop before parents. Placeholder entries with no
/// `drop_fn` are skipped.
///
/// # Safety
///
/// - `payload` must be the payload region of a live chunk.
/// - `drop_entry_count` must equal the number of `DropEntry`s previously
///   written by the allocator at the tail.
/// - The first `value_offset + len * size_of::<T>()` bytes of `payload`
///   referenced by each committed entry must contain `len` initialized `T`s
///   where `T` matches the type baked into the entry's `drop_fn` shim.
/// - The caller must own the chunk exclusively (refcount zero); replay
///   mutates payload bytes via type-erased `drop_in_place`.
#[allow(
    clippy::cast_ptr_alignment,
    reason = "caller guarantees entries are naturally aligned within the payload; see DropEntry layout"
)]
pub(crate) unsafe fn replay_drops(payload: *mut u8, payload_len: usize, drop_entry_count: usize) {
    if drop_entry_count == 0 {
        return;
    }
    let entry_size = mem::size_of::<DropEntry>();
    let entry_align = mem::align_of::<DropEntry>();
    // Align the absolute payload end so entry positions stay valid even when
    // the payload start is not `entry_align`-aligned.
    let payload_addr = payload as usize;
    let aligned_end_offset = ((payload_addr.wrapping_add(payload_len)) & !(entry_align - 1)).wrapping_sub(payload_addr);
    // Entries grow downward, so reverse index order visits newest -> oldest.
    for i in (0..drop_entry_count).rev() {
        let entry_off = aligned_end_offset - (i + 1) * entry_size;
        // SAFETY: `entry_off + entry_size <= aligned_end_offset <= payload_len`,
        // so the entry lies inside the payload. The caller guarantees the
        // entry and any committed value range are initialized and type-matched.
        let entry = &*(payload.add(entry_off).cast::<DropEntry>());
        if let Some(shim) = entry.drop_fn() {
            let value_off = entry.value_offset() as usize;
            let count = entry.len() as usize;
            shim(payload.add(value_off), count);
        }
    }
}

#[cfg(test)]
#[allow(clippy::cast_ptr_alignment, reason = "test buffer is manually aligned")]
mod tests {
    use super::*;

    /// [`replay_drops`] must locate entries by absolute payload-end alignment
    /// even when `payload_ptr` is not `DropEntry`-aligned. Only committed
    /// entries run.
    #[test]
    fn replay_tolerates_unaligned_payload_start() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn counting_shim(_p: *mut u8, _n: usize) {
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        CALLS.store(0, Ordering::Relaxed);

        let entry_size = mem::size_of::<DropEntry>();
        let entry_align = mem::align_of::<DropEntry>();
        // Buffer big enough to host two entries plus the misalignment slack.
        let extra = entry_align - 1;
        let payload_len = entry_size * 2 + extra;
        // Over-allocate by `entry_align` so we can choose an unaligned start.
        let mut buf = std::vec![0u8; payload_len + entry_align];
        let base_addr = buf.as_mut_ptr() as usize;
        // Pick a payload_start address that's odd-aligned. Anchor 1 byte
        // past an aligned base so payload_addr mod entry_align == 1.
        let aligned_base = (base_addr + entry_align - 1) & !(entry_align - 1);
        let payload_start_addr = aligned_base + 1;
        let payload_offset = payload_start_addr - base_addr;
        // SAFETY: `payload_offset + payload_len` ≤ `buf.len()` by construction.
        let payload_ptr = unsafe { buf.as_mut_ptr().add(payload_offset) };
        assert_ne!((payload_ptr as usize) % entry_align, 0, "payload must be unaligned for this test");

        // Where the entries *must* land: at the absolute-aligned end.
        let aligned_end_addr = (payload_start_addr + payload_len) & !(entry_align - 1);
        let aligned_end_offset = aligned_end_addr - payload_start_addr;

        let shim_fn = counting_shim as DropFn;

        // Write a committed entry and a non-committed placeholder at the
        // correctly-aligned offsets.
        // SAFETY: both offsets are within the payload buffer and produce
        // entry_align-aligned addresses by construction.
        unsafe {
            let top_off = aligned_end_offset - entry_size;
            let next_off = aligned_end_offset - 2 * entry_size;
            // Top: placeholder left uncommitted (no shim).
            ptr::write(payload_ptr.add(top_off).cast::<DropEntry>(), DropEntry::placeholder(99, 1));
            // Below: placeholder committed to the counting shim.
            let next_ptr = payload_ptr.add(next_off).cast::<DropEntry>();
            ptr::write(next_ptr, DropEntry::placeholder(0, 1));
            (*next_ptr).commit_drop_fn(shim_fn);
        }

        // Only the committed shim runs.
        // SAFETY: payload_ptr + payload_len bounds the live buffer.
        unsafe { replay_drops(payload_ptr, payload_len, 2) };
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    }

    /// `raw_used` returns the unpadded field-size sum.
    #[test]
    fn raw_used_is_sum_of_field_sizes() {
        let expected = mem::size_of::<DropFn>() + mem::size_of::<u16>() + mem::size_of::<u16>();
        assert_eq!(raw_used(), expected);
        // On 64-bit targets: 8 + 2 + 2 = 12.
        #[cfg(target_pointer_width = "64")]
        assert_eq!(raw_used(), 12);
    }

    /// `pad_bytes` rounds `raw_used()` up to `PAD_TARGET` so the
    /// composite `DropEntry` aligns to its function-pointer field.
    #[test]
    fn pad_bytes_aligns_to_pad_target() {
        let pad = pad_bytes();
        let total = raw_used() + pad;
        assert_eq!(total % PAD_TARGET, 0, "raw + pad must be multiple of PAD_TARGET");
        // `pad < PAD_TARGET` because rounding up adds at most
        // `PAD_TARGET - 1` bytes; equal-to-zero when already aligned.
        assert!(pad < PAD_TARGET);
        // PAD_BYTES is the compile-time evaluation of pad_bytes().
        assert_eq!(PAD_BYTES, pad);
    }
}
