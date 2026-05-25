// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Trailing drop-list machinery.
//!
//! The drop list is a back-stack of [`DropEntry`] records that grows
//! down from the end of a chunk's payload while ordinary forward
//! allocations grow up from the start. When a chunk is being torn
//! down (refcount reached zero) it pops the back-stack
//! most-recent-first and invokes each `(drop_fn)(value, len)` shim,
//! then frees its backing allocation.

use core::mem;
// `DropEntry::drop_fn` uses `core::sync::atomic::AtomicPtr` directly
// (not the loom shim) because the chunk's bump allocator writes
// freshly-constructed `DropEntry` values into chunk memory via raw
// `core::ptr::write`. Loom atomics are address-keyed in the model
// checker's runtime; moving them via byte-copy invalidates that
// tracking. The cross-thread `assume_init` retarget Release on this
// field is still indirectly modeled by loom via the Release/Acquire
// chain on the chunk's `refcount` and `drop_count` (both routed
// through the loom shim), which gates all `replay_drops` reads.
use core::sync::atomic::{AtomicPtr, Ordering};

/// Round `min_payload` up so `(header_size + payload)` is a multiple
/// of `align_of::<DropEntry>()` — required for the back-stack write
/// at `data + capacity - size_of::<DropEntry>()` to land on an
/// aligned address (data lives at offset `header_size` within a
/// CHUNK_ALIGN-aligned base, so the alignment requirement reduces
/// to the sum being aligned).
///
/// Callers pass `local_chunk::header_size::<A>()` or
/// `shared_chunk::header_size::<A>()`. Budget reservation and chunk
/// allocation must use the same rounded value or `total_chunk_bytes`
/// drifts and eventually wraps below zero.
///
/// Returns `None` on `usize` overflow.
#[inline]
pub(crate) const fn round_payload(min_payload: usize, header_size: usize) -> Option<usize> {
    let entry_align = mem::align_of::<DropEntry>();
    let mask = entry_align - 1;
    let Some(total) = header_size.checked_add(min_payload) else {
        return None;
    };
    let Some(rounded_total) = total.checked_add(mask) else {
        return None;
    };
    Some((rounded_total & !mask) - header_size)
}

/// A single entry in a chunk's trailing drop list.
///
/// The shim is generated per `T` at the allocation site, so replaying
/// the drop list can call the concrete `drop_in_place::<T>` (or
/// `drop_in_place::<[T]>`) without more type information.
///
/// The shim pointer is stored as an [`AtomicPtr`] so concurrent
/// retargeting from cross-thread converters
/// (e.g. `Arc::<MaybeUninit<T>>::assume_init` on cloned handles) is
/// well-defined; otherwise two threads could non-atomically write the
/// same field, which is a data race even when the values are bitwise
/// identical.
#[repr(C)]
pub(crate) struct DropEntry {
    /// Type-erased shim; for `len == 1` it drops a single `T`, for
    /// `len > 1` it drops a `[T]` of `len` elements.
    ///
    /// Stored as an [`AtomicPtr`] so writes from `assume_init` /
    /// `into_rc` retargeting are race-free even when concurrent
    /// `Arc<MaybeUninit<T>>` clones call `assume_init` simultaneously.
    /// Use [`DropEntry::store_drop_fn`] / [`DropEntry::load_drop_fn`]
    /// for ordered access; the raw field is `pub(crate)` only so the
    /// initial whole-struct write from `core::ptr::write` works.
    ///
    /// # Safety contract on the shim
    ///
    /// The shim is paired with a value that lives at
    /// `chunk_data + value_offset`, of type matching the shim's
    /// monomorphization, with `len` elements. Each shim is only ever
    /// invoked once per [`DropEntry`].
    pub(crate) drop_fn: AtomicPtr<()>,

    /// Byte offset of the value (or first slice element) within the
    /// chunk's payload. Bounded by 64 KiB for cached chunks, hence
    /// `u16`.
    pub(crate) value_offset: u16,

    /// Number of `T`s starting at `value_offset`. `1` for ordinary
    /// single-value entries.
    pub(crate) len: u16,

    /// Padding to a pointer-aligned slot so successive entries on the
    /// back-stack remain naturally aligned.
    _pad: [u8; PAD_BYTES],
}

const PAD_TARGET: usize = mem::align_of::<unsafe fn(*mut u8, usize)>();

#[cfg_attr(test, mutants::skip)] // Padding arithmetic mutations are hidden by Rust struct alignment rounding.
#[cfg_attr(coverage_nightly, coverage(off))]
const fn raw_used() -> usize {
    mem::size_of::<unsafe fn(*mut u8, usize)>() + 2 + 2
}

#[cfg_attr(test, mutants::skip)] // Padding arithmetic mutations are hidden by Rust struct alignment rounding.
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

impl DropEntry {
    #[inline]
    pub(crate) const fn new(drop_fn: unsafe fn(*mut u8, usize), value_offset: u16, len: u16) -> Self {
        Self {
            #[expect(
                clippy::fn_to_numeric_cast_any,
                reason = "fn-ptr storage in AtomicPtr is the type-erased shim location"
            )]
            drop_fn: AtomicPtr::new(drop_fn as *mut ()),
            value_offset,
            len,
            _pad: [0; PAD_BYTES],
        }
    }

    /// Atomically store a new `drop_fn`. Use `Release` when publishing
    /// for cross-thread observers (e.g. shared chunks); `Relaxed` is
    /// fine for local-only chunks.
    #[inline]
    #[expect(
        clippy::fn_to_numeric_cast_any,
        reason = "fn-ptr storage in AtomicPtr is the type-erased shim location"
    )]
    pub(crate) fn store_drop_fn(&self, drop_fn: unsafe fn(*mut u8, usize), order: Ordering) {
        self.drop_fn.store(drop_fn as *mut (), order);
    }

    /// Atomically load `drop_fn`. Replay sites can use `Relaxed` because
    /// the chunk's `refcount` → 0 release/acquire fence already orders
    /// all prior stores from any thread before the replay.
    #[inline]
    pub(crate) fn load_drop_fn(&self, order: Ordering) -> unsafe fn(*mut u8, usize) {
        let p = self.drop_fn.load(order);
        // SAFETY: every store originates from a valid `unsafe fn(*mut u8, usize)`
        // cast at construction or via `store_drop_fn`.
        unsafe { mem::transmute::<*mut (), unsafe fn(*mut u8, usize)>(p) }
    }

    /// Write `self` as a fresh entry at `data_ptr + byte_offset`. Used
    /// by allocation paths that already proved the back-stack slot is
    /// inside the live chunk payload and naturally aligned for
    /// [`DropEntry`].
    ///
    /// # Safety
    ///
    /// `data_ptr + byte_offset` must point at writable, owned,
    /// `DropEntry`-aligned bytes inside a live chunk (the back-stack
    /// slot the caller just reserved), and the slot must not currently
    /// hold an initialized entry (the write is non-drop / overwriting
    /// memory).
    #[inline]
    pub(crate) unsafe fn write_at_offset(self, data_ptr: core::ptr::NonNull<u8>, byte_offset: usize) {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "back-stack slots are placed at naturally-aligned addresses by the bump allocator (CHUNK_ALIGN >= align_of::<DropEntry>())"
        )]
        // SAFETY: caller proves the slot is in-payload, aligned, and writable.
        unsafe {
            let entry_ptr = data_ptr.as_ptr().add(byte_offset).cast::<Self>();
            core::ptr::write(entry_ptr, self);
        }
    }
}

/// No-op shim for allocations whose storage is intentionally uninitialized.
///
/// # Safety
///
/// Accepts any pointer/length pair and deliberately does nothing.
pub(crate) unsafe fn noop_drop_shim(_value: *mut u8, _len: usize) {}

/// Generate the type-erased drop shim for a single value of type `T`.
///
/// # Safety
///
/// `value` must point at an initialized `T` and `len` must be `1`.
pub(crate) unsafe fn drop_shim_one<T>(value: *mut u8, len: usize) {
    debug_assert_eq!(len, 1, "single-value drop shim invoked with len != 1");
    // SAFETY: drop-shim invariant — the value at `value` is a valid
    // `T` and is dropped exactly once.
    unsafe { core::ptr::drop_in_place::<T>(value.cast::<T>()) };
}

/// Generate the type-erased drop shim for a `[T]` of `len` elements.
///
/// # Safety
///
/// - `value` must point at the first of `len` initialized `T`s.
/// - `len * size_of::<T>()` must not exceed `isize::MAX` (the upper
///   bound on a single allocation's byte length). In-tree this holds
///   because every caller's `len <= u16::MAX` and the chunk-payload
///   size is capped at `MAX_NORMAL_ALLOC` for normal allocations and
///   at `MAX_CHUNK_BYTES` for oversized ones.
pub(crate) unsafe fn drop_shim_slice<T>(value: *mut u8, len: usize) {
    let slice: *mut [T] = core::ptr::slice_from_raw_parts_mut(value.cast::<T>(), len);
    // SAFETY: drop-shim invariant — the slice covers `len` valid `T`s
    // and is dropped exactly once.
    unsafe { core::ptr::drop_in_place::<[T]>(slice) };
}

/// Drop guard used to force-abort the process if a wrapped `T::Drop`
/// invocation unwinds during `replay_drops`. Under `no_std` there is
/// no `core::panic::catch_unwind` (it's `std`-only) and
/// `core::panic::abort_unwind` is unstable, so the only way to keep
/// chunk reclamation correct in the face of a panicking `T::Drop` is
/// to abort the process via double-panic (panic-in-drop while already
/// unwinding causes `panic = unwind` to abort). For `panic = abort`
/// builds, the original panic itself already aborts and this guard is
/// never observed.
///
/// The caller is expected to `core::mem::forget(guard)` after a
/// successful drop-shim call to suppress the destructor.
///
/// The type and its `Drop` are compiled unconditionally so the abort
/// semantics are testable under `std` builds via
/// `std::panic::catch_unwind`; only the in-tree call sites are
/// gated on `not(feature = "std")`.
#[cfg_attr(feature = "std", allow(dead_code, reason = "exercised via tests; live call sites are no_std-only"))]
pub(crate) struct AbortOnUnwind;

impl Drop for AbortOnUnwind {
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // Fatal-abort path: double-panic terminates the process, untestable in-process.
    fn drop(&mut self) {
        #[expect(clippy::panic, reason = "fatal-path abort: double-panic forces process termination")]
        {
            panic!("multitude: T::Drop panicked during replay_drops; aborting to prevent chunk leak");
        }
    }
}

/// Iterate the trailing drop list at `(data, capacity)` and invoke each
/// recorded drop-shim. Shared between [`LocalChunk::replay_drops`] and
/// [`SharedChunk::replay_drops`] (their per-flavor parts cover the
/// `drop_count` read / reset and the chunk-liveness contract).
///
/// Each call is wrapped panic-safely: under `std` with `catch_unwind`
/// (so one panicking `T::Drop` leaks only its own value, not the
/// chunk), under `no_std` with an [`AbortOnUnwind`] guard (no
/// `catch_unwind`; the only way to keep chunk reclamation correct is
/// to escalate the panic to a process abort via double-panic).
///
/// # Safety
///
/// * `data` must be the chunk's payload base and remain valid for
///   reads for `capacity` bytes (caller-owned chunk).
/// * The last `count * size_of::<DropEntry>()` bytes of the payload
///   must hold `count` initialized `DropEntry`s installed by
///   [`crate::arena::primitives`] (densely packed, naturally aligned).
/// * Each entry's `(value_offset, len, drop_fn)` must satisfy the
///   drop-shim invariant: `data + value_offset` points at `len`
///   initialized values of the type `drop_fn` was synthesized for.
/// * The caller must have reset the on-chunk `drop_count` to zero
///   before invoking, so a panicking `Drop` cannot leave stale entries
///   to be replayed twice.
pub(crate) unsafe fn replay_drop_entries(data: *mut u8, capacity: usize, count: usize) {
    if count == 0 {
        return;
    }
    let base = capacity - count * mem::size_of::<DropEntry>();
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "chunk payloads are CHUNK_ALIGN aligned; `data + base` is naturally `DropEntry`-aligned by construction"
    )]
    // SAFETY: `base = capacity - count * size_of::<DropEntry>()` is
    // the byte offset of the first entry; together with `data` it
    // addresses `count` initialized `DropEntry`s.
    let entries_ptr = unsafe { data.add(base).cast::<DropEntry>() };
    for i in 0..count {
        // SAFETY: `i < count`; all `count` entries are initialized at allocation time.
        let entry = unsafe { &*entries_ptr.add(i) };
        // SAFETY: payload-extent + drop-shim invariants.
        let value_ptr = unsafe { data.add(entry.value_offset as usize) };
        let f = entry.load_drop_fn(Ordering::Relaxed);
        #[cfg(feature = "std")]
        {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: drop-shim invariant.
                unsafe { f(value_ptr, entry.len as usize) };
            }));
        }
        #[cfg(not(feature = "std"))]
        {
            let abort_guard = AbortOnUnwind;
            // SAFETY: drop-shim invariant.
            unsafe { f(value_ptr, entry.len as usize) };
            core::mem::forget(abort_guard);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_payload_overflow_returns_none() {
        // header_size = 0 plus `min_payload = usize::MAX` plus the
        // alignment mask still overflows, exercising the inner
        // `checked_add` branch.
        assert!(round_payload(usize::MAX, 0).is_none());
    }

    #[test]
    fn round_payload_header_plus_payload_overflow_returns_none() {
        // `header_size + min_payload` itself overflows here (any
        // non-zero header pushes `usize::MAX` past the limit),
        // exercising the outer `checked_add` branch that returns
        // `None` before the alignment rounding step.
        assert!(round_payload(usize::MAX, 1).is_none());
    }

    #[test]
    fn round_payload_zero_rounds_to_zero() {
        // header_size = 0 keeps the original meaning: round payload
        // to a multiple of `align_of::<DropEntry>()`.
        assert_eq!(round_payload(0, 0), Some(0));
    }

    #[test]
    fn round_payload_rounds_up_to_alignment() {
        let align = mem::align_of::<DropEntry>();
        // DropEntry is composed of a fn pointer + two u16s + pointer-sized padding,
        // so its alignment is always > 1 on supported targets.
        assert!(align > 1);
        assert_eq!(round_payload(1, 0), Some(align));
    }

    #[test]
    fn round_payload_compensates_for_header_misalignment() {
        let align = mem::align_of::<DropEntry>();
        // With a header that's 2 mod `align`, payload should be
        // rounded so that `header + payload` becomes a multiple of
        // `align`. For align = 8 and header = 2, payload = 0 needs
        // padding to 6; payload = 1 still needs padding to 6.
        let header = 2;
        let payload = round_payload(1, header).expect("no overflow");
        assert_eq!((header + payload) % align, 0);
        assert!(payload >= 1);
    }

    #[test]
    fn drop_entry_layout_matches_natural_alignment() {
        // `DropEntry` packs a fn pointer (8 bytes on 64-bit) plus two u16s
        // plus padding to the fn-pointer alignment. Any mutation of
        // `RAW_USED` / `PAD_BYTES` arithmetic in the const block above
        // would alter `size_of::<DropEntry>()`, breaking the drop-list
        // stack walker that assumes consecutive entries are
        // `size_of::<DropEntry>()` apart.
        let fn_align = mem::align_of::<unsafe fn(*mut u8, usize)>();
        let raw = mem::size_of::<unsafe fn(*mut u8, usize)>() + 2 + 2;
        let expected = raw.div_ceil(fn_align) * fn_align;
        assert_eq!(mem::size_of::<DropEntry>(), expected);
        assert_eq!(mem::align_of::<DropEntry>(), fn_align);
    }

    /// Dropping an `AbortOnUnwind` guard must panic — that is what
    /// turns a panicking `T::Drop` during `replay_drops` into a
    /// double-panic process abort under `no_std`. We assert the
    /// `Drop` impl panics; a mutant that replaces the body with `()`
    /// would let `catch_unwind` return `Ok` and fail this test.
    #[cfg(feature = "std")]
    #[test]
    fn abort_on_unwind_drop_panics() {
        let result = std::panic::catch_unwind(|| {
            let _guard = AbortOnUnwind;
        });
        assert!(result.is_err(), "AbortOnUnwind::drop must panic");
    }
}
