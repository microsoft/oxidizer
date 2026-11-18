// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc arena.c and bitmap.c; see LICENSE-MIMALLOC.

//! Per-arena indexes for abandoned pages.
//!
//! A set bit publishes a page that an allocation can reclaim. Clearing the bit
//! pins the page while the reader tries to acquire its remote-list ownership.
//! A failed reader restores the bit. A freeing owner clears the bit only once
//! it is set, so metadata cannot be destroyed beneath a reader.

use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use crate::backend::mimalloc::classes::SLICE;
use crate::backend::mimalloc::memory::{self, Memory};
use crate::backend::mimalloc::{atomic, hal, pagemap};

const ARENA_BINS: usize = 61; // _mi_bin(MI_LARGE_MAX_OBJ_SIZE) + 1.
const MAX_ARENAS: usize = 160; // types.h:MI_MAX_ARENAS.
const CHUNK_BITS: usize = 512;
const WORD_BITS: usize = usize::BITS as usize;
const CHUNK_WORDS: usize = CHUNK_BITS / WORD_BITS;

#[repr(align(64))]
struct Chunk {
    words: [AtomicUsize; CHUNK_WORDS],
}

impl Chunk {
    const fn new() -> Self {
        Self {
            words: [const { AtomicUsize::new(0) }; CHUNK_WORDS],
        }
    }

    fn is_empty(&self) -> bool {
        self.words.iter().all(|word| word.load(Ordering::Relaxed) == 0)
    }

    fn set(&self, index: usize) {
        self.words[index / WORD_BITS].fetch_or(1 << (index % WORD_BITS), Ordering::Release);
    }

    fn try_clear(&self, index: usize) -> bool {
        let mask = 1 << (index % WORD_BITS);
        let word = &self.words[index / WORD_BITS];
        atomic::try_update(word, Ordering::AcqRel, Ordering::Acquire, |previous| {
            (previous & mask != 0).then_some(previous & !mask)
        })
        .is_ok()
    }

    fn take_first(&self) -> Option<usize> {
        for (index, word) in self.words.iter().enumerate() {
            let taken = atomic::try_update(word, Ordering::AcqRel, Ordering::Relaxed, |bits| {
                bits.checked_sub(1).map(|smaller| bits & smaller)
            });
            if let Ok(bits) = taken {
                return Some(index * WORD_BITS + bits.trailing_zeros() as usize);
            }
        }
        None
    }
}

struct Bitmap<'a> {
    summary: Chunk,
    chunks: &'a [Chunk],
}

impl<'a> Bitmap<'a> {
    const fn new(chunks: &'a [Chunk]) -> Self {
        Self {
            summary: Chunk::new(),
            chunks,
        }
    }

    fn publish(&self, index: usize) {
        let chunk = index / CHUNK_BITS;
        self.chunks[chunk].set(index % CHUNK_BITS);
        self.summary.set(chunk);
    }

    fn remove(&self, index: usize) {
        self.remove_with_wait(index, std::thread::yield_now);
    }

    fn remove_with_wait(&self, index: usize, mut waiting: impl FnMut()) {
        let chunk = &self.chunks[index / CHUNK_BITS];
        while !chunk.try_clear(index % CHUNK_BITS) {
            waiting();
        }
    }

    fn clear_empty_summary(&self, index: usize) {
        self.clear_empty_summary_with(index, || {});
    }

    fn clear_empty_summary_with(&self, index: usize, after_empty_check: impl FnOnce()) {
        let chunk = &self.chunks[index];
        if chunk.is_empty() {
            after_empty_check();
            self.summary.try_clear(index);
            // A publisher can race between the empty check and clearing the
            // summary. Repeat the check so its notification cannot be lost.
            if !chunk.is_empty() {
                self.summary.set(index);
            }
        }
    }

    fn find(&self, sequence: usize, mut claim: impl FnMut(usize) -> bool) -> Option<usize> {
        for (word_index, word) in self.summary.words.iter().enumerate() {
            let bits = word.load(Ordering::Relaxed);
            if bits == 0 {
                continue;
            }
            // bitmap.c rotates within the accessed part, with eight thread
            // starting positions to limit spreading across additional chunks.
            let cycle = WORD_BITS - bits.leading_zeros() as usize;
            let start = (sequence % 8) % cycle;
            let mut high = bits & (usize::MAX << start);
            let mut low = bits & !(usize::MAX << start);
            while high != 0 || low != 0 {
                if high == 0 {
                    high = low;
                    low = 0;
                }
                let chunk = word_index * WORD_BITS + high.trailing_zeros() as usize;
                high &= high - 1;
                if let Some(bit) = self.chunks[chunk].take_first() {
                    let index = chunk * CHUNK_BITS + bit;
                    let pin = Reader { bitmap: self, index };
                    if claim(index) {
                        // Page ownership replaces the temporary bitmap pin.
                        std::mem::forget(pin);
                        return Some(index);
                    }
                    drop(pin);
                } else {
                    self.clear_empty_summary(chunk);
                }
            }
        }
        None
    }
}

struct Reader<'a, 'b> {
    bitmap: &'a Bitmap<'b>,
    index: usize,
}

impl Drop for Reader<'_, '_> {
    fn drop(&mut self) {
        self.bitmap.publish(self.index);
    }
}

struct ArenaPages {
    base: usize,
    maps: [Bitmap<'static>; ARENA_BINS],
    _reservation: hal::Reservation,
}

// SAFETY: Publication freezes the owning reservation. Only the atomic maps
// change afterward, and no shared operation releases or mutates the reservation.
unsafe impl Sync for ArenaPages {}

struct UnpublishedDirectory(*mut ArenaPages);

impl Drop for UnpublishedDirectory {
    fn drop(&mut self) {
        // SAFETY: Construction follows header initialization. Publication
        // consumes this guard, so only unpublished, exclusively owned headers
        // reach this destructor. The owning reservation is the last drop field.
        unsafe { self.0.drop_in_place() };
    }
}

static ARENAS: [AtomicPtr<ArenaPages>; MAX_ARENAS] = [const { AtomicPtr::new(std::ptr::null_mut()) }; MAX_ARENAS];
static COUNTS: [AtomicUsize; ARENA_BINS] = [const { AtomicUsize::new(0) }; ARENA_BINS];

pub(super) fn observe() -> [u64; seismograph_rallocator::mimalloc::CLASS_COUNT] {
    const _: () = assert!(ARENA_BINS == seismograph_rallocator::mimalloc::ABANDONED_CLASS_COUNT);
    std::array::from_fn(|bin| COUNTS.get(bin).map_or(0, |count| count.load(Ordering::Relaxed) as u64))
}

/// A page's immutable location in the abandoned index.
pub(super) struct Registration {
    arena: Option<&'static ArenaPages>,
    slice: usize,
    bin: usize,
}

impl Registration {
    pub(super) fn new(memory: &Memory, bin: usize) -> Option<Self> {
        if bin < ARENA_BINS
            && let Some((index, slice, count)) = memory.arena_location()
        {
            let base = memory.as_ptr().addr() - slice * SLICE;
            // SAFETY: Memory supplies the immutable geometry of this process's
            // persistent arena. Its index is never reused for another arena.
            let arena = unsafe { arena_pages(index, count, base) }?;
            return Some(Self {
                arena: Some(arena),
                slice,
                bin,
            });
        }
        Some(Self {
            arena: None,
            slice: 0,
            bin,
        })
    }

    /// # Safety
    /// For an arena registration, the page is initialized, not already indexed,
    /// and owned by the caller. It stays registered until removal or reclaim.
    pub(super) unsafe fn publish(&self) -> bool {
        let Some(arena) = self.arena else { return false };
        // The publisher retains page ownership until both stores complete.
        // No reader can successfully reclaim the page before that release.
        COUNTS[self.bin].fetch_add(1, Ordering::Relaxed);
        arena.maps[self.bin].publish(self.slice);
        true
    }

    pub(super) fn remove(&self) {
        if let Some(arena) = self.arena {
            arena.maps[self.bin].remove(self.slice);
            COUNTS[self.bin].fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// # Safety
/// The geometry identifies the persistent arena at this directory index.
unsafe fn arena_pages(index: usize, slices: usize, base: usize) -> Option<&'static ArenaPages> {
    let slot = ARENAS.get(index)?;
    let existing = slot.load(Ordering::Acquire);
    if !existing.is_null() {
        // SAFETY: Published directories and their metadata stay mapped.
        return Some(unsafe { &*existing });
    }
    let commit = |reservation: &hal::Reservation| {
        // SAFETY: This fresh metadata reservation is exclusively owned.
        unsafe { reservation.commit(0, reservation.len()) }
    };
    // SAFETY: The private slot contains only a valid permanent directory, and
    // the callback commits this call's fresh, exclusively owned reservation.
    unsafe { create_arena_pages(slot, slices, base, commit, || {}) }
}

/// # Safety
/// The slot is empty or contains a permanent valid directory for this arena.
/// A successful commit callback must make the whole reservation writable.
/// The publication callback must not publish this reservation or invalidate
/// the slot or its independently owned directory.
unsafe fn create_arena_pages(
    slot: &AtomicPtr<ArenaPages>,
    slices: usize,
    base: usize,
    commit: impl FnOnce(&hal::Reservation) -> bool,
    before_publication: impl FnOnce(),
) -> Option<&'static ArenaPages> {
    let chunks = slices.div_ceil(CHUNK_BITS);
    if chunks == 0 || chunks > CHUNK_BITS {
        return None;
    }
    let offset = std::mem::size_of::<ArenaPages>().next_multiple_of(std::mem::align_of::<Chunk>());
    let size = offset.checked_add(chunks.checked_mul(ARENA_BINS * std::mem::size_of::<Chunk>())?)?;
    let reservation = hal::reserve(size, SLICE)?;
    if !commit(&reservation) {
        return None;
    }
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "The reservation and offset are multiples of 64 KiB and 64 bytes"
    )]
    let storage = reservation.as_ptr().wrapping_add(offset).cast::<Chunk>();
    for index in 0..chunks * ARENA_BINS {
        // SAFETY: Every aligned, disjoint chunk is within committed metadata.
        unsafe { storage.wrapping_add(index).write(Chunk::new()) };
    }
    // SAFETY: All chunks were initialized above. The successful directory
    // publication retains this reservation for the process lifetime.
    let storage = unsafe { std::slice::from_raw_parts(storage, chunks * ARENA_BINS) };
    #[expect(clippy::cast_ptr_alignment, reason = "The metadata reservation is 64-KiB aligned")]
    let pointer = reservation.as_ptr().cast::<ArenaPages>();
    let value = ArenaPages {
        base,
        maps: std::array::from_fn(|bin| Bitmap::new(&storage[bin * chunks..(bin + 1) * chunks])),
        _reservation: reservation,
    };
    // SAFETY: The header is aligned, committed, and does not overlap its chunks.
    unsafe { pointer.write(value) };
    let unpublished = UnpublishedDirectory(pointer);
    before_publication();
    let published = match slot.compare_exchange(std::ptr::null_mut(), pointer, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {
            std::mem::forget(unpublished);
            pointer
        }
        Err(existing) => {
            drop(unpublished);
            existing
        }
    };
    // SAFETY: The winning directory is initialized and permanently retained.
    Some(unsafe { &*published })
}

/// The callback must replace a bitmap reader's pin with page ownership before
/// returning true. On false, it must stop using the page before it returns.
///
/// # Safety
/// The callback must obey the pin-to-ownership transfer described above.
pub(super) unsafe fn reclaim(bin: usize, sequence: usize, mut claim: impl FnMut(usize) -> bool) -> Option<usize> {
    if bin >= ARENA_BINS || COUNTS[bin].load(Ordering::Relaxed) == 0 {
        return None;
    }
    let mut result = None;
    let arenas = memory::arena_order(memory::arena_count(), sequence)
        .filter_map(|index| std::ptr::NonNull::new(ARENAS[index].load(Ordering::Acquire)));
    for pointer in arenas {
        // SAFETY: Published metadata directories stay mapped.
        let arena = unsafe { pointer.as_ref() };
        let found = arena.maps[bin].find(sequence, |slice| {
            // SAFETY: Clearing the abandoned bit pins the page and its map
            // entry until this reader restores the bit or acquires ownership.
            let address = unsafe { pagemap::lookup(arena.base + slice * SLICE) };
            if claim(address) {
                result = Some(address);
                true
            } else {
                false
            }
        });
        if found.is_some() {
            COUNTS[bin].fetch_sub(1, Ordering::Relaxed);
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Barrier, mpsc};

    use super::{Bitmap, CHUNK_BITS, Chunk};

    #[test]
    fn failed_page_ownership_restores_the_abandoned_entry() {
        let chunks = [Chunk::new(), Chunk::new()];
        let bitmap = Bitmap::new(&chunks);
        bitmap.publish(17);
        bitmap.publish(CHUNK_BITS + 4);

        assert_eq!(bitmap.find(0, |_| false), None);
        assert_eq!(bitmap.find(0, |_| true), Some(17));
        assert_eq!(bitmap.find(0, |_| true), Some(CHUNK_BITS + 4));
        assert_eq!(bitmap.find(0, |_| true), None);
    }

    #[test]
    fn search_rotates_across_accessed_chunks() {
        let chunks = [Chunk::new(), Chunk::new(), Chunk::new()];
        let bitmap = Bitmap::new(&chunks);
        for index in [9, CHUNK_BITS + 3, 2 * CHUNK_BITS + 1] {
            bitmap.publish(index);
        }

        assert_eq!(bitmap.find(1, |_| true), Some(CHUNK_BITS + 3));
        assert_eq!(bitmap.find(2, |_| true), Some(2 * CHUNK_BITS + 1));
        assert_eq!(bitmap.find(2, |_| true), Some(9));
    }

    #[test]
    fn removing_owner_waits_for_a_failed_reader_to_restore_its_pin() {
        let chunks = [Chunk::new()];
        let bitmap = Bitmap::new(&chunks);
        bitmap.publish(31);
        let rendezvous = Barrier::new(2);
        let removed = AtomicBool::new(false);
        let (reader_paused, paused) = mpsc::channel();
        let (owner_waiting, waiting) = mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                assert_eq!(
                    bitmap.find(0, |_| {
                        reader_paused.send(()).unwrap();
                        rendezvous.wait();
                        assert!(!removed.load(Ordering::Acquire));
                        false
                    }),
                    None
                );
            });
            paused.recv().unwrap();
            scope.spawn(|| {
                let mut first_wait = true;
                bitmap.remove_with_wait(31, || {
                    if first_wait {
                        first_wait = false;
                        owner_waiting.send(()).unwrap();
                    }
                    std::hint::spin_loop();
                });
                removed.store(true, Ordering::Release);
            });
            let waited = waiting.recv_timeout(std::time::Duration::from_secs(5));
            rendezvous.wait();
            waited.unwrap();
        });

        assert!(removed.load(Ordering::Acquire));
        assert_eq!(bitmap.find(0, |_| true), None);
    }

    #[test]
    fn concurrent_claimers_cannot_receive_the_same_page() {
        let chunks = [Chunk::new(), Chunk::new()];
        let bitmap = Bitmap::new(&chunks);
        for index in 0..2 * CHUNK_BITS {
            bitmap.publish(index);
        }

        let results = std::thread::scope(|scope| {
            let first = scope.spawn(|| std::iter::from_fn(|| bitmap.find(0, |_| true)).collect::<Vec<_>>());
            let second = scope.spawn(|| std::iter::from_fn(|| bitmap.find(1, |_| true)).collect::<Vec<_>>());
            let mut results = first.join().unwrap();
            results.extend(second.join().unwrap());
            results
        });
        let mut results = results;
        results.sort_unstable();
        assert_eq!(results, (0..2 * CHUNK_BITS).collect::<Vec<_>>());
    }

    #[test]
    fn a_panicking_reader_restores_its_abandoned_entry() {
        let chunks = [Chunk::new()];
        let bitmap = Bitmap::new(&chunks);
        bitmap.publish(23);

        let result = std::panic::catch_unwind(|| bitmap.find(0, |_| panic!("reader interrupted")));

        result.unwrap_err();
        assert_eq!(bitmap.find(0, |_| true), Some(23));
    }

    #[test]
    fn summary_clear_cannot_erase_a_concurrent_publication() {
        let chunks = [Chunk::new()];
        let bitmap = Bitmap::new(&chunks);

        bitmap.clear_empty_summary_with(0, || bitmap.publish(27));
        bitmap.clear_empty_summary(0);

        assert_eq!(bitmap.find(0, |_| true), Some(27));
    }

    #[test]
    fn os_pages_have_no_abandoned_arena_entry() {
        let registration = super::Registration {
            arena: None,
            slice: 0,
            bin: super::ARENA_BINS,
        };

        // SAFETY: Unindexed OS pages publish no bitmap entry.
        assert!(!unsafe { registration.publish() });
        registration.remove();
    }

    #[test]
    fn metadata_dimensions_reject_empty_and_oversized_arenas() {
        let slot = super::AtomicPtr::new(std::ptr::null_mut());
        let results = [0, CHUNK_BITS * CHUNK_BITS + 1].map(
            |slices|             // SAFETY: Rejected dimensions return before any callback or slot access.
            unsafe { super::create_arena_pages(&slot, slices, 0, |_| unreachable!(), || {}) }.is_none(),
        );

        assert_eq!(results, [true, true]);
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn failed_metadata_commit_does_not_publish_a_directory() {
        let slot = super::AtomicPtr::new(std::ptr::null_mut());

        // SAFETY: The local slot is empty, the commit callback reports failure,
        // and the publication callback does not change it.
        let result = unsafe { super::create_arena_pages(&slot, CHUNK_BITS, 0, |_| false, || {}) };

        assert!(result.is_none());
        assert!(slot.load(Ordering::Acquire).is_null());
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn interrupted_metadata_creation_does_not_publish_a_directory() {
        let slot = super::AtomicPtr::new(std::ptr::null_mut());
        let commit = |reservation: &super::hal::Reservation| {
            // SAFETY: This unpublished reservation is exclusively owned.
            unsafe { reservation.commit(0, reservation.len()) }
        };

        let result = std::panic::catch_unwind(|| {
            // SAFETY: The local slot is empty. The commit callback makes the
            // entire range writable, and the interruption does not alter it.
            unsafe { super::create_arena_pages(&slot, CHUNK_BITS, 0, commit, || panic!("creation interrupted")) }
        });

        assert!(result.is_err());
        assert!(slot.load(Ordering::Acquire).is_null());
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn simultaneous_metadata_creation_publishes_one_directory() {
        let slot = super::AtomicPtr::new(std::ptr::null_mut());
        let ready = Barrier::new(2);
        let commit = |reservation: &super::hal::Reservation| {
            // SAFETY: Each caller exclusively owns this unpublished reservation.
            unsafe { reservation.commit(0, reservation.len()) }
        };
        let addresses = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                // SAFETY: Both creators publish only their initialized permanent
                // directory. The barrier does not alter either reservation.
                unsafe {
                    super::create_arena_pages(&slot, CHUNK_BITS, 0, commit, || {
                        ready.wait();
                    })
                }
                .unwrap()
            });
            let second = scope.spawn(|| {
                // SAFETY: The same local slot and committing callback are used.
                unsafe {
                    super::create_arena_pages(&slot, CHUNK_BITS, 0, commit, || {
                        ready.wait();
                    })
                }
                .unwrap()
            });
            [
                std::ptr::from_ref(first.join().unwrap()),
                std::ptr::from_ref(second.join().unwrap()),
            ]
        });

        assert_eq!(addresses[0], addresses[1]);
        let pointer = slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: Both creators have joined and the local slot no longer
        // publishes this directory. Move its owning field before releasing it.
        let directory = unsafe { &*pointer };
        #[expect(
            clippy::used_underscore_binding,
            reason = "The field only retains ownership in production; this test releases its local directory"
        )]
        // SAFETY: No reader, static root, or destructor can access this field again.
        let reservation = unsafe { std::ptr::from_ref(&directory._reservation).read() };
        drop(reservation);
    }
}
