// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See ../LICENSE-MIMALLOC.

//! Atomic slice ownership and the two-level, size-binned search from bitmap.c.
//!
//! A clear free bit means that an allocation, a purge, or a partial claim owns
//! the slice. Multi-word failures restore only the bits that attempt acquired.
//! Free-bit release publishes commit changes to the next acquiring claimant.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// Upstream uses eight 64-bit words per chunk, and at most 512 chunks.
pub(in super::super) const CHUNK_SLICES: usize = 512;
pub(in super::super) const MAX_SLICES: usize = CHUNK_SLICES * CHUNK_SLICES;
const WORDS: usize = MAX_SLICES / 64;
const NONE: usize = 0;
const SMALL: usize = 1;
const OTHER: usize = 2;
const MEDIUM: usize = 3;
const LARGE: usize = 4;
const HUGE: usize = 5;

/// All-zero bytes form a valid empty bitmap, for initialization in OS memory.
/// The C layout keeps every eight-word chunk on a cache-line boundary.
#[repr(C, align(64))]
pub(in super::super) struct Bitmap<const N: usize = WORDS> {
    free: [AtomicU64; N],
    committed: [AtomicU64; N],
    pending: [AtomicU64; N],
    dirty: [AtomicU64; N],
    summary: [AtomicU64; 8],
    bin_maps: [[AtomicU64; 8]; 5],
    max_accessed: AtomicUsize,
    slices: usize,
}

/// Exclusive ownership of a free-bitmap range, including partially committed
/// backing. Dropping the guard publishes that range as free again.
pub(in super::super) struct Claim<'a, const N: usize = WORDS> {
    bitmap: &'a Bitmap<N>,
    start: usize,
    count: usize,
    armed: bool,
}

impl<const N: usize> Bitmap<N> {
    pub(in super::super) fn observe(&self, budget: &mut usize) -> (u64, u64, u64, bool) {
        let mut counts = (0, 0, 0);
        for index in 0..self.slices / 64 {
            if *budget == 0 {
                return (counts.0, counts.1, counts.2, false);
            }
            *budget -= 1;
            counts.0 += u64::from(self.free[index].load(Ordering::Relaxed).count_ones());
            counts.1 += u64::from(self.committed[index].load(Ordering::Relaxed).count_ones());
            counts.2 += u64::from(self.pending[index].load(Ordering::Relaxed).count_ones());
        }
        (counts.0, counts.1, counts.2, true)
    }

    #[cfg(test)]
    pub(in super::super) const fn zeroed() -> Self {
        Self {
            free: [const { AtomicU64::new(0) }; N],
            committed: [const { AtomicU64::new(0) }; N],
            pending: [const { AtomicU64::new(0) }; N],
            dirty: [const { AtomicU64::new(0) }; N],
            summary: [const { AtomicU64::new(0) }; 8],
            bin_maps: [const { [const { AtomicU64::new(0) }; 8] }; 5],
            max_accessed: AtomicUsize::new(0),
            slices: 0,
        }
    }

    pub(in super::super) fn initialize(&mut self, slices: usize, eager: bool) {
        assert!(slices > 0 && slices <= MAX_SLICES && slices <= N * 64 && slices.is_multiple_of(CHUNK_SLICES));
        self.slices = slices;
        modify(&self.free, 0, slices, true);
        modify(&self.committed, 0, slices, eager);
        for chunk in 0..slices / CHUNK_SLICES {
            self.summary[chunk / 64].fetch_or(1 << (chunk % 64), Ordering::Relaxed);
        }
    }

    pub(in super::super) fn reserve_metadata(&mut self, start: usize, count: usize) {
        assert!(count > 0 && start <= self.slices && count <= self.slices - start);
        let mut offset = start;
        let mut remaining = count;
        while remaining != 0 {
            let width = remaining.min(64 - offset % 64);
            *self.free[offset / 64].get_mut() &= !(mask(width) << (offset % 64));
            offset += width;
            remaining -= width;
        }
        for chunk in start / CHUNK_SLICES..=(start + count - 1) / CHUNK_SLICES {
            self.clear_summary_if_full(chunk);
        }
    }

    pub(in super::super) fn metadata_committed(&self, start: usize, count: usize) -> bool {
        all(&self.committed, start, count)
    }

    pub(in super::super) fn set_metadata_committed(&self, start: usize, count: usize) {
        modify(&self.committed, start, count, true);
    }

    fn assign_bin(&self, chunk: usize, bin: usize) {
        let mask = 1 << (chunk % 64);
        for (index, map) in self.bin_maps.iter().enumerate() {
            if index + 1 == bin {
                map[chunk / 64].fetch_or(mask, Ordering::AcqRel);
            } else {
                map[chunk / 64].fetch_and(!mask, Ordering::AcqRel);
            }
        }
    }

    fn publish_free(&self, start: usize, count: usize) {
        modify(&self.free, start, count, true);
        self.restore_summary(start, count, true);
    }

    fn restore_summary(&self, start: usize, count: usize, check_empty: bool) {
        for chunk in start / CHUNK_SLICES..=(start + count - 1) / CHUNK_SLICES {
            if check_empty && all(&self.free, chunk * CHUNK_SLICES, CHUNK_SLICES) {
                self.assign_bin(chunk, NONE);
            }
            self.summary[chunk / 64].fetch_or(1 << (chunk % 64), Ordering::AcqRel);
            self.max_accessed.fetch_max(chunk, Ordering::Relaxed);
        }
    }

    fn clear_summary_if_full(&self, chunk: usize) {
        self.clear_summary_with(chunk, || {});
    }

    fn clear_summary_with(&self, chunk: usize, before_clear: impl FnOnce()) {
        let fields = &self.free[chunk * 8..chunk * 8 + 8];
        if fields.iter().any(|w| w.load(Ordering::Relaxed) != 0) {
            return;
        }
        before_clear();
        let mask = 1 << (chunk % 64);
        self.summary[chunk / 64].fetch_and(!mask, Ordering::AcqRel);
        // A release can occur between the first check and the summary clear.
        // The acquiring clear and second check prevent losing that notification.
        if fields.iter().any(|w| w.load(Ordering::Relaxed) != 0) {
            self.summary[chunk / 64].fetch_or(mask, Ordering::AcqRel);
        }
        self.max_accessed.fetch_max(chunk, Ordering::Relaxed);
    }

    fn claim_at(&self, start: usize, count: usize) -> Option<Claim<'_, N>> {
        assert!(count > 0 && start <= self.slices && count <= self.slices - start);
        if !try_clear(&self.free, start, count) {
            // Temporary clears can race with a summary clear. Republish all
            // affected chunks even when the failed attempt restored no bits.
            self.restore_summary(start, count, false);
            return None;
        }
        for chunk in start / CHUNK_SLICES..=(start + count - 1) / CHUNK_SLICES {
            self.clear_summary_if_full(chunk);
        }
        Some(Claim {
            bitmap: self,
            start,
            count,
            armed: true,
        })
    }

    /// # Safety
    /// The caller transfers its exclusive ownership of an allocated range.
    /// No other claim guard or Memory owner can release this range.
    pub(in super::super) unsafe fn reclaim(&self, start: usize, count: usize) -> Claim<'_, N> {
        assert!(count > 0 && start <= self.slices && count <= self.slices - start);
        Claim {
            bitmap: self,
            start,
            count,
            armed: true,
        }
    }

    pub(in super::super) fn claim(&self, count: usize, sequence: usize) -> Option<Claim<'_, N>> {
        if count == 0 || count > self.slices {
            return None;
        }
        if count > CHUNK_SLICES {
            return self.claim_singleton(count);
        }
        let bin = match count {
            1 => SMALL,
            8 => MEDIUM,
            64 => LARGE,
            _ => OTHER,
        };
        let accessed = self.max_accessed.load(Ordering::Relaxed);
        let groups = self.slices.div_ceil(CHUNK_SLICES * 64);
        for group in cycle_bits(mask(groups), sequence, accessed / 64 + 1) {
            let available = self.summary[group].load(Ordering::Relaxed);
            let mut bins = [0; 6];
            bins[NONE] = available;
            for index in SMALL..=HUGE {
                bins[index] = self.bin_maps[index - 1][group].load(Ordering::Relaxed) & available;
                bins[NONE] &= !bins[index];
            }
            // Larger pages can consume chunks assigned to smaller bins, never
            // the reverse. Unassigned chunks are considered last in each group.
            let cycle = if group == accessed / 64 { accessed % 64 + 1 } else { 64 };
            for wanted in (SMALL..=bin).chain(std::iter::once(NONE)) {
                for bit in cycle_bits(bins[wanted], sequence, cycle) {
                    let chunk = group * 64 + bit;
                    if let Some(claim) = self.claim_chunk(chunk, count) {
                        if claim.start % CHUNK_SLICES == 0 && wanted == NONE {
                            self.assign_bin(chunk, bin);
                        }
                        return Some(claim);
                    }
                    self.clear_summary_if_full(chunk);
                }
            }
        }
        None
    }

    fn claim_chunk(&self, chunk: usize, count: usize) -> Option<Claim<'_, N>> {
        loop {
            let words = std::array::from_fn::<_, 8, _>(|i| self.free[chunk * 8 + i].load(Ordering::Relaxed));
            let offset = find_run(&words, count)?;
            if let Some(claim) = self.claim_at(chunk * CHUNK_SLICES + offset, count) {
                return Some(claim);
            }
            // The failed word claim restores its prefix before we scan again.
            // Like the reference allocator's optimistic loop, contention retries the chunk.
        }
    }

    fn claim_singleton(&self, count: usize) -> Option<Claim<'_, N>> {
        // Upstream starts singleton searches at chunk boundaries and requires
        // every covering chunk to be free, including the final partial chunk.
        let chunks = count.div_ceil(CHUNK_SLICES);
        for chunk in 0..=self.slices / CHUNK_SLICES - chunks {
            let start = chunk * CHUNK_SLICES;
            if all(&self.free, start, chunks * CHUNK_SLICES)
                && let Some(claim) = self.claim_at(start, count)
            {
                for index in chunk..chunk + chunks {
                    self.assign_bin(index, HUGE);
                }
                return Some(claim);
            }
        }
        None
    }

    /// Consumes each pending word atomically, preserving notifications that
    /// arrive after its snapshot. The purge callback always holds a free claim.
    pub(in super::super) fn purge_pending(&self, minimum: usize, mut purge: impl FnMut(&Claim<'_, N>) -> bool) -> bool {
        self.purge_pending_with(minimum, &mut purge)
    }

    fn purge_pending_with(&self, minimum: usize, purge: &mut dyn FnMut(&Claim<'_, N>) -> bool) -> bool {
        assert!(minimum > 0 && minimum <= 64 && minimum.is_power_of_two());
        let mut any_claimed = false;
        for word in 0..self.slices / 64 {
            let mut snapshot = self.pending[word].swap(0, Ordering::AcqRel);
            if minimum == 1 {
                while snapshot != 0 {
                    let bit = snapshot.trailing_zeros() as usize;
                    let count = (snapshot >> bit).trailing_ones() as usize;
                    snapshot &= !(mask(count) << bit);
                    any_claimed |= self.purge_candidate(word * 64 + bit, count, purge);
                }
            } else {
                let mut skipped = 0;
                for bit in (0..64).step_by(minimum) {
                    let bits = mask(minimum) << bit;
                    if snapshot & bits == bits {
                        any_claimed |= self.purge_candidate(word * 64 + bit, minimum, purge);
                    } else {
                        skipped |= snapshot & bits;
                    }
                }
                self.pending[word].fetch_or(skipped, Ordering::AcqRel);
            }
        }
        any_claimed
    }

    fn purge_candidate(&self, start: usize, count: usize, purge: &mut dyn FnMut(&Claim<'_, N>) -> bool) -> bool {
        if let Some(claim) = self.claim_at(start, count) {
            if purge(&claim) {
                claim.set_committed(false);
            }
            return true;
        }
        // Upstream retries individual slices when an allocation prevents the
        // full pending range from being claimed, even for a THP-sized group.
        let mut any_claimed = false;
        if count > 1 {
            for slice in start..start + count {
                if let Some(claim) = self.claim_at(slice, 1) {
                    if purge(&claim) {
                        claim.set_committed(false);
                    }
                    any_claimed = true;
                }
            }
        }
        any_claimed
    }
}

impl<const N: usize> Claim<'_, N> {
    pub(in super::super) const fn start(&self) -> usize {
        self.start
    }

    pub(in super::super) const fn count(&self) -> usize {
        self.count
    }

    pub(in super::super) fn committed(&self) -> bool {
        all(&self.bitmap.committed, self.start, self.count)
    }

    pub(in super::super) fn set_committed(&self, value: bool) {
        modify(&self.bitmap.committed, self.start, self.count, value);
    }

    pub(in super::super) fn schedule_purge(&self) {
        modify(&self.bitmap.pending, self.start, self.count, true);
    }

    /// Marks all claimed slices dirty, even if a later commit fails. Returns
    /// whether every slice was untouched, as in the `slices_dirty` bitmap in `arena.c`.
    pub(in super::super) fn mark_dirty(&self) -> bool {
        let mut start = self.start;
        let mut count = self.count;
        let mut zero = true;
        while count != 0 {
            let width = count.min(64 - start % 64);
            let bits = mask(width) << (start % 64);
            zero &= self.bitmap.dirty[start / 64].fetch_or(bits, Ordering::AcqRel) & bits == 0;
            start += width;
            count -= width;
        }
        zero
    }

    /// Transfers the claim to the Memory owner that records this index.
    pub(in super::super) fn into_index(mut self) -> usize {
        self.armed = false;
        self.start
    }
}

impl<const N: usize> Drop for Claim<'_, N> {
    fn drop(&mut self) {
        if self.armed {
            self.bitmap.publish_free(self.start, self.count);
        }
    }
}

fn mask(count: usize) -> u64 {
    u64::MAX >> (64 - count)
}

fn modify(words: &[AtomicU64], mut start: usize, mut count: usize, value: bool) {
    while count != 0 {
        let width = count.min(64 - start % 64);
        let bits = mask(width) << (start % 64);
        if value {
            words[start / 64].fetch_or(bits, Ordering::AcqRel);
        } else {
            words[start / 64].fetch_and(!bits, Ordering::AcqRel);
        }
        start += width;
        count -= width;
    }
}

fn all(words: &[AtomicU64], mut start: usize, mut count: usize) -> bool {
    while count != 0 {
        let width = count.min(64 - start % 64);
        let bits = mask(width) << (start % 64);
        if words[start / 64].load(Ordering::Acquire) & bits != bits {
            return false;
        }
        start += width;
        count -= width;
    }
    true
}

fn try_clear(words: &[AtomicU64], start: usize, count: usize) -> bool {
    let mut offset = start;
    let mut remaining = count;
    while remaining != 0 {
        let width = remaining.min(64 - offset % 64);
        let bits = mask(width) << (offset % 64);
        let word = &words[offset / 64];
        // Full fields use the source's compare-exchange path. Partial fields
        // optimistically clear their mask only after a favorable initial read.
        let cleared = if bits == u64::MAX {
            word.compare_exchange(u64::MAX, 0, Ordering::AcqRel, Ordering::Relaxed).is_ok()
        } else {
            word.load(Ordering::Relaxed) & bits == bits && optimistic_clear(word, bits)
        };
        if !cleared {
            modify(words, start, offset - start, true);
            return false;
        }
        offset += width;
        remaining -= width;
    }
    true
}

fn optimistic_clear(word: &AtomicU64, bits: u64) -> bool {
    let previous = word.fetch_and(!bits, Ordering::AcqRel);
    if previous & bits == bits {
        return true;
    }
    // Restore only the bits this atomic attempt actually acquired. Other
    // owners' clear bits must never become free as part of our rollback.
    word.fetch_or(previous & bits, Ordering::AcqRel);
    false
}

fn cycle_bits(bits: u64, sequence: usize, cycle: usize) -> impl Iterator<Item = usize> {
    // bitmap.c truncates the thread sequence to 32 bits before rotation.
    let start = (sequence & 0xFFFF_FFFF) % cycle;
    let first_mask = mask(cycle - start) << start;
    let mut first = bits & first_mask;
    let mut rest = bits & !first_mask;
    std::iter::from_fn(move || {
        if first == 0 {
            first = rest;
            rest = 0;
        }
        if first == 0 {
            return None;
        }
        let bit = first.trailing_zeros() as usize;
        first &= first - 1;
        Some(bit)
    })
}

fn find_run(words: &[u64], count: usize) -> Option<usize> {
    // The common 1- and 8-slice paths use a bitscan, with byte alignment
    // for the latter. Other ranges can cross words, but never chunks.
    for (index, &word) in words.iter().enumerate() {
        if count <= 64 {
            let mut candidates = word;
            let mut shift = 1;
            while shift < count {
                candidates &= candidates >> shift.min(count - shift);
                shift *= 2;
            }
            let alignment_mask = if count == 8 { 0x0101_0101_0101_0101 } else { u64::MAX };
            candidates &= alignment_mask;
            if candidates != 0 {
                return Some(index * 64 + candidates.trailing_zeros() as usize);
            }
        }
        if count == 1 || count == 8 {
            continue;
        }
        let suffix = word.leading_ones() as usize;
        let start = index * 64 + 64 - suffix;
        if suffix > 0 && start + count <= words.len() * 64 {
            let mut offset = start;
            let mut remaining = count;
            while remaining != 0 {
                let width = remaining.min(64 - offset % 64);
                let bits = mask(width) << (offset % 64);
                if words[offset / 64] & bits != bits {
                    break;
                }
                offset += width;
                remaining -= width;
            }
            if remaining == 0 {
                return Some(start);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn observation_reads_free_commit_and_pending_bits_without_claiming_slices() {
        let mut bitmap = super::Bitmap::<8>::zeroed();
        bitmap.initialize(512, false);
        bitmap.reserve_metadata(0, 2);
        bitmap.set_metadata_committed(0, 2);
        let claimed = bitmap.claim(8, 0).unwrap();
        claimed.set_committed(true);
        claimed.schedule_purge();
        drop(claimed);

        let full = bitmap.observe(&mut 8);
        let partial = bitmap.observe(&mut 1);
        let again = bitmap.observe(&mut 8);

        assert_eq!((full, partial, again), ((510, 10, 8, true), (62, 10, 8, false), (510, 10, 8, true)));
    }

    use std::cell::Cell;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{Bitmap, CHUNK_SLICES, cycle_bits, find_run, optimistic_clear};

    fn bitmap() -> Bitmap<24> {
        let mut bitmap = Bitmap::zeroed();
        bitmap.initialize(CHUNK_SLICES * 3, false);
        bitmap
    }

    #[test]
    fn ordinary_pages_reuse_smaller_bins_without_fragmenting_larger_bins() {
        let bitmap = bitmap();
        let medium = bitmap.claim(8, 0).unwrap();
        let small = bitmap.claim(1, 0).unwrap();
        let large = bitmap.claim(64, 0).unwrap();

        assert_eq!((medium.start(), small.start(), large.start()), (0, 512, 513));
    }

    #[test]
    fn byte_groups_skip_misaligned_runs() {
        assert_eq!(find_run(&[0x0000_0000_0001_ff80_u64], 8), Some(8));
    }

    #[test]
    fn singleton_run_crosses_word_boundaries_without_word_alignment() {
        assert_eq!(find_run(&[u64::MAX << 63, u64::MAX, 0], 65), Some(63));
    }

    #[test]
    fn a_cross_word_run_is_rejected_when_the_next_word_has_a_gap() {
        assert_eq!(find_run(&[1 << 63, 0], 2), None);
    }

    #[test]
    fn singleton_release_restores_full_and_partial_chunks() {
        let bitmap = bitmap();
        let first = bitmap.claim(513, 0).unwrap();
        let start = first.start();
        drop(first);
        let reused = bitmap.claim(1024, 0).unwrap();

        assert_eq!((start, reused.start()), (0, 0));
    }

    #[test]
    fn metadata_is_skipped_when_finding_aligned_medium_pages() {
        let mut bitmap = bitmap();
        bitmap.reserve_metadata(0, 5);
        let page = bitmap.claim(8, 0).unwrap();

        assert_eq!(page.start(), 8);
    }

    #[test]
    fn singleton_pages_cannot_span_reserved_metadata() {
        let mut bitmap = bitmap();
        bitmap.reserve_metadata(512, 1);

        assert!(bitmap.claim(513, 0).is_none());
    }

    #[test]
    fn purging_data_never_changes_permanent_metadata_commitment() {
        let mut bitmap = bitmap();
        bitmap.reserve_metadata(0, 8);
        bitmap.set_metadata_committed(0, 8);
        let page = bitmap.claim(8, 0).unwrap();
        page.set_committed(true);
        page.schedule_purge();
        drop(page);
        let mut ranges = Vec::new();

        bitmap.purge_pending(1, |claim| {
            ranges.push((claim.start(), claim.count()));
            true
        });

        assert_eq!((ranges, bitmap.metadata_committed(0, 8)), (vec![(8, 8)], true));
    }

    #[test]
    fn zero_tracking_remembers_dirty_slices_across_word_boundaries() {
        let bitmap = bitmap();
        let first = bitmap.claim_at(63, 65).unwrap();
        let first_zero = first.mark_dirty();
        drop(first);
        let reused = bitmap.claim_at(64, 1).unwrap();
        let untouched = bitmap.claim_at(0, 1).unwrap();

        assert_eq!((first_zero, reused.mark_dirty(), untouched.mark_dirty()), (true, false, true));
    }

    #[test]
    fn failed_commits_return_ownership_without_resetting_dirty_bits() {
        let bitmap = bitmap();
        let first = bitmap.claim(8, 0).unwrap();
        let first_zero = first.mark_dirty();
        drop(first);
        let retry = bitmap.claim(8, 0).unwrap();

        assert_eq!((first_zero, retry.mark_dirty(), retry.committed()), (true, false, false));
    }

    #[test]
    fn a_later_metadata_failure_keeps_successful_data_commits_for_retry() {
        let bitmap = bitmap();
        let first = bitmap.claim(8, 0).unwrap();
        first.mark_dirty();
        first.set_committed(true);
        drop(first);
        let retry = bitmap.claim(8, 0).unwrap();

        assert_eq!((retry.start(), retry.mark_dirty(), retry.committed()), (0, false, true));
    }

    #[test]
    fn cycle_visits_rotated_accessed_bits_before_unvisited_tail() {
        assert_eq!(cycle_bits(0b11_1111, 2, 4).collect::<Vec<_>>(), [2, 3, 0, 1, 4, 5]);
    }

    #[test]
    fn threads_start_searching_in_different_accessed_chunks() {
        let bitmap = bitmap();
        bitmap.assign_bin(1, super::SMALL);
        bitmap.assign_bin(2, super::SMALL);
        bitmap.max_accessed.store(2, Ordering::Relaxed);
        let first = bitmap.claim(1, 1).unwrap();
        let second = bitmap.claim(1, 2).unwrap();

        assert_eq!((first.start(), second.start()), (512, 1024));
    }

    #[test]
    fn failed_word_claim_does_not_restore_bits_owned_by_another_claim() {
        let bitmap = bitmap();
        let owner = bitmap.claim_at(4, 4).unwrap();
        let failed = bitmap.claim_at(0, 8).is_none();
        let prefix = bitmap.claim_at(0, 4).unwrap();
        let still_owned = bitmap.claim_at(4, 4).is_none();

        assert_eq!((failed, prefix.start(), still_owned, owner.start()), (true, 0, true, 4));
    }

    #[test]
    fn optimistic_race_restores_only_previously_free_bits() {
        let word = AtomicU64::new(0b1010_0101);
        let success = optimistic_clear(&word, 0xFF);

        assert_eq!((success, word.load(Ordering::Acquire)), (false, 0b1010_0101));
    }

    #[test]
    fn simultaneous_claims_of_one_word_have_one_owner() {
        let bitmap = bitmap();
        let barrier = Barrier::new(3);
        let winners = std::thread::scope(|scope| {
            let handles = std::array::from_fn::<_, 2, _>(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let claim = bitmap.claim_at(0, 64);
                    barrier.wait();
                    claim.is_some()
                })
            });
            barrier.wait();
            barrier.wait();
            handles.map(|handle| handle.join().unwrap())
        });

        assert_eq!(winners.into_iter().filter(|&won| won).count(), 1);
    }

    #[test]
    fn a_release_during_summary_clear_is_not_lost() {
        let bitmap = bitmap();
        let owner = bitmap.claim_at(0, CHUNK_SLICES).unwrap();
        owner.into_index();
        // SAFETY: The test splits its exclusive chunk ownership into disjoint
        // ranges. Neither range has any other guard or Memory owner.
        let released = unsafe { bitmap.reclaim(0, 1) };
        // SAFETY: This range is disjoint from the single-slice release above.
        let _remaining = unsafe { bitmap.reclaim(1, CHUNK_SLICES - 1) };

        bitmap.clear_summary_with(0, || drop(released));
        let reused = bitmap.claim(1, 0).unwrap();

        assert_eq!(reused.start(), 0);
    }

    #[test]
    fn invalid_claim_sizes_do_not_change_free_slices() {
        let bitmap = bitmap();
        let invalid = (bitmap.claim(0, 0).is_none(), bitmap.claim(1537, 0).is_none());
        let valid = bitmap.claim(8, 0).unwrap();

        assert_eq!((invalid, valid.start()), ((true, true), 0));
    }

    #[test]
    fn exhausted_arenas_report_no_free_range() {
        let bitmap = bitmap();
        let _owners = std::array::from_fn::<_, 3, _>(|_| bitmap.claim(CHUNK_SLICES, 0).unwrap());

        assert!(bitmap.claim(1, 0).is_none());
    }

    #[test]
    fn stale_summary_bits_are_cleared_before_search_continues() {
        let bitmap = bitmap();
        let _owner = bitmap.claim_at(0, CHUNK_SLICES).unwrap();
        bitmap.summary[0].fetch_or(1, Ordering::AcqRel);

        let next = bitmap.claim(1, 0).unwrap();

        assert_eq!((next.start(), bitmap.summary[0].load(Ordering::Acquire) & 1), (CHUNK_SLICES, 0));
    }

    #[test]
    fn search_reaches_a_new_chunk_map_word() {
        let mut bitmap = Bitmap::<520>::zeroed();
        bitmap.initialize(65 * CHUNK_SLICES, false);
        bitmap.reserve_metadata(0, 64 * CHUNK_SLICES);

        let page = bitmap.claim(1, 0).unwrap();
        let no_large_run = bitmap.claim(CHUNK_SLICES + 1, 0).is_none();

        assert_eq!((page.start(), no_large_run), (64 * CHUNK_SLICES, true));
    }

    #[test]
    fn contended_tail_rolls_back_only_the_claimed_prefix() {
        let bitmap = bitmap();
        let barrier = Barrier::new(2);
        let result = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let tail = bitmap.claim_at(64, 64).unwrap();
                tail.set_committed(true);
                barrier.wait();
                barrier.wait();
                tail.start()
            });
            barrier.wait();
            let failed = bitmap.claim_at(63, 65).is_none();
            let prefix = bitmap.claim_at(0, 64).unwrap();
            let prefix_start = prefix.start();
            let tail_owned = bitmap.claim_at(64, 64).is_none();
            barrier.wait();
            (failed, prefix_start, tail_owned, worker.join().unwrap())
        });
        let reused = bitmap.claim_at(0, 128).unwrap();

        assert_eq!((result, reused.committed(), reused.start()), ((true, 0, true, 64), false, 0));
    }

    #[test]
    fn concurrent_byte_claims_fill_a_chunk_without_overlap() {
        let mut bitmap = bitmap();
        bitmap.slices = CHUNK_SLICES;
        let barrier = Barrier::new(4);
        let claims = std::thread::scope(|scope| {
            let handles = std::array::from_fn::<_, 4, _>(|sequence| {
                let bitmap = &bitmap;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    std::array::from_fn::<_, 16, _>(|_| bitmap.claim(8, sequence).unwrap())
                })
            });
            handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect::<Vec<_>>()
        });
        let mut starts = claims.iter().map(super::Claim::start).collect::<Vec<_>>();
        starts.sort_unstable();

        assert_eq!(starts, (0..512).step_by(8).collect::<Vec<_>>());
    }

    #[test]
    fn purge_holds_an_exclusive_claim_until_commit_bits_change() {
        let bitmap = bitmap();
        let claim = bitmap.claim(8, 0).unwrap();
        claim.set_committed(true);
        claim.schedule_purge();
        drop(claim);
        let mut excluded = false;
        bitmap.purge_pending(1, |claim| {
            excluded = bitmap.claim_at(claim.start(), claim.count()).is_none();
            true
        });
        let reused = bitmap.claim(8, 0).unwrap();

        assert_eq!((excluded, reused.start(), reused.committed()), (true, 0, false));
    }

    #[test]
    fn allocation_wins_a_race_with_a_pending_purge() {
        let bitmap = bitmap();
        let claim = bitmap.claim(8, 0).unwrap();
        claim.set_committed(true);
        claim.schedule_purge();
        drop(claim);
        let reused = bitmap.claim(8, 0).unwrap();
        let purged = Cell::new(0);
        let mut purge = |_: &super::Claim<'_, 24>| {
            purged.set(purged.get() + 1);
            true
        };
        bitmap.purge_pending(1, &mut purge);
        let busy = (reused.start(), reused.committed(), purged.get());
        drop(reused);
        let released = bitmap.claim_at(0, 8).unwrap();
        released.schedule_purge();
        drop(released);
        bitmap.purge_pending(1, &mut purge);

        assert_eq!((busy, purged.get()), ((0, true, 0), 1));
    }

    #[test]
    fn a_busy_single_slice_is_not_purged_until_a_new_notification() {
        let bitmap = bitmap();
        let first = bitmap.claim(1, 0).unwrap();
        first.set_committed(true);
        first.schedule_purge();
        drop(first);
        let busy = bitmap.claim(1, 0).unwrap();
        let calls = Cell::new(0);
        let mut purge = |_: &super::Claim<'_, 24>| {
            calls.set(calls.get() + 1);
            true
        };

        bitmap.purge_pending(1, &mut purge);
        let before_release = (calls.get(), busy.committed());
        drop(busy);
        let released = bitmap.claim(1, 0).unwrap();
        released.schedule_purge();
        drop(released);
        bitmap.purge_pending(1, &mut purge);

        assert_eq!((before_release, calls.get()), ((0, true), 1));
    }

    #[test]
    fn new_purge_notifications_survive_the_current_word_snapshot() {
        let bitmap = bitmap();
        let claim = bitmap.claim_at(0, 8).unwrap();
        claim.schedule_purge();
        drop(claim);
        bitmap.purge_pending(1, |_| {
            let newer = bitmap.claim_at(8, 8).unwrap();
            newer.schedule_purge();
            true
        });

        assert_eq!(bitmap.pending[0].load(Ordering::Acquire), 0xFF00);
    }

    #[test]
    fn retained_accessible_pages_keep_their_committed_state_after_purge() {
        let bitmap = bitmap();
        let claim = bitmap.claim(64, 0).unwrap();
        claim.set_committed(true);
        claim.schedule_purge();
        drop(claim);
        bitmap.purge_pending(1, |_| false);
        let reused = bitmap.claim(64, 0).unwrap();

        assert_eq!((reused.committed(), bitmap.pending[0].load(Ordering::Acquire)), (true, 0));
    }

    #[test]
    fn thp_purge_retains_incomplete_pending_groups() {
        let bitmap = bitmap();
        let claim = bitmap.claim(8, 0).unwrap();
        claim.schedule_purge();
        drop(claim);
        let purged = Cell::new(false);
        let mut purge = |_: &super::Claim<'_, 24>| {
            purged.set(true);
            true
        };
        bitmap.purge_pending(32, &mut purge);
        let incomplete = (purged.get(), bitmap.pending[0].load(Ordering::Acquire));
        let rest = bitmap.claim_at(8, 24).unwrap();
        rest.schedule_purge();
        drop(rest);
        bitmap.purge_pending(32, &mut purge);

        assert_eq!(
            (incomplete, purged.get(), bitmap.pending[0].load(Ordering::Acquire)),
            ((false, 0xFF), true, 0)
        );
    }

    #[test]
    fn a_reused_slice_does_not_prevent_purging_its_free_neighbors() {
        let bitmap = bitmap();
        let first = bitmap.claim(64, 0).unwrap();
        first.set_committed(true);
        first.schedule_purge();
        drop(first);
        let busy = bitmap.claim_at(8, 8).unwrap();
        let mut purged = 0;

        bitmap.purge_pending(32, |claim| {
            purged += claim.count();
            true
        });

        assert_eq!((purged, busy.committed(), bitmap.pending[0].load(Ordering::Acquire)), (56, true, 0));
    }

    #[test]
    fn partial_range_purge_can_retain_commitment_for_every_free_neighbor() {
        let bitmap = bitmap();
        let page = bitmap.claim(64, 0).unwrap();
        page.set_committed(true);
        page.schedule_purge();
        drop(page);
        let busy = bitmap.claim_at(8, 8).unwrap();
        let mut visited = 0;

        bitmap.purge_pending(32, |claim| {
            visited += claim.count();
            false
        });

        assert_eq!((visited, busy.committed(), bitmap.metadata_committed(0, 64)), (56, true, true));
    }

    #[test]
    fn dropping_an_uncommitted_claim_returns_it_without_scheduling_purge() {
        let bitmap = bitmap();
        let claim = bitmap.claim(64, 0).unwrap();
        let start = claim.start();
        drop(claim);
        let reused = bitmap.claim(64, 0).unwrap();

        assert_eq!(
            (reused.start(), reused.committed(), bitmap.pending[0].load(Ordering::Acquire)),
            (start, false, 0)
        );
    }
}
