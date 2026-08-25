// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Primitives for thread-aware data storage.

#[cfg(not(test))]
use alloc::boxed::Box;
#[cfg(not(test))]
use alloc::vec::Vec;
use std::marker::PhantomData;
use std::num::NonZero;
use std::sync::{self, OnceLock};

use nm::Event;

use crate::affinity::Affinity;

/// Maps affinities into strategy partitions for storage.
///
/// A strategy assigns each affinity a partition index and reports how many partitions the storage
/// holds. The
/// affinities whose values share one thread-aware [`Arc`](crate::Arc) are expected to map into a
/// single fixed coordinate space: every such affinity reports the same partition count, and its
/// index falls within that count. The built-in strategies satisfy this because the counts are
/// machine properties — the processor and memory-region counts — that do not vary across the
/// affinities of one machine.
pub trait Strategy {
    /// Returns the strategy partition index for the given affinity.
    fn index(affinity: Affinity) -> usize;

    /// Returns the number of strategy partitions the storage holds.
    ///
    /// The count is at least one and is expected to be the same for every affinity whose value shares
    /// one thread-aware [`Arc`](crate::Arc), because the storage is sized to it exactly once.
    fn count(affinity: Affinity) -> NonZero<usize>;
}

/// Rejects a direct [`Storage`] access whose affinity falls outside the storage's coordinate space.
///
/// [`Storage::insert`] and [`Storage::get`] require an affinity the storage was sized for. An
/// affinity indexing past the partition count is a caller error — the caller mixed coordinate spaces
/// rather than relying on the degraded relocation path — so the access panics rather than silently
/// discarding a value or returning a value from an unrelated partition. Split into its own `#[cold]`
/// function so the panic machinery stays off the inlined accessor bodies.
#[cold]
#[expect(
    clippy::panic,
    reason = "documented panic path: direct Storage access with a mismatched coordinate space is a caller error"
)]
fn out_of_coordinate_space(index: usize, partition_count: usize) -> ! {
    panic!(
        "Storage affinity index {index} is outside its coordinate space (partition count: {partition_count}); direct Storage access requires affinities that map into the partition count this storage was sized for"
    );
}

/// Strategy-partitioned storage shared by every clone of an [`Arc`](crate::Arc).
///
/// A relocation into an affinity publishes a value for the affinity's strategy partition; later
/// relocations into any affinity that maps to the same partition read it back. This can also be
/// built directly and populated with [`insert`](Self::insert), then handed to
/// [`Arc::from_storage`](crate::Arc::from_storage) to produce an `Arc` backed by values prepared for
/// specific strategy partitions.
#[derive(Debug)]
pub struct Storage<T: ?Sized, S: Strategy> {
    // Storage fixes the stored type to `sync::Arc<T>`; SlotTable remains value-agnostic so its
    // write-once behavior can be tested with plain values.
    inner: SlotTable<sync::Arc<T>, S>,
}

impl<T: ?Sized, S: Strategy> Storage<T, S> {
    /// Creates an empty storage, with no strategy partition populated.
    #[must_use]
    pub const fn new() -> Self {
        Self { inner: SlotTable::new() }
    }

    /// Publishes the value for `affinity`'s strategy partition if it is still empty.
    ///
    /// Each strategy partition is written at most once, so a value is stored only when the
    /// partition was empty: `Ok(())` is returned in that case. When the partition already holds a
    /// value, the passed-in `value` is handed back as `Err(value)`. This mirrors
    /// [`OnceLock::set`](std::sync::OnceLock::set) rather than the previous-value semantics of
    /// [`HashMap::insert`](std::collections::HashMap::insert).
    ///
    /// # Errors
    ///
    /// Returns `Err(value)`, handing the passed-in `value` back unchanged, when the strategy
    /// partition selected by `affinity` already holds a value.
    ///
    /// # Panics
    ///
    /// Panics if `affinity` falls outside the storage's coordinate space — an index at or beyond the
    /// partition count the strategy reports. Direct callers are expected to use affinities the
    /// storage was sized for.
    #[inline]
    pub fn insert(&self, affinity: Affinity, value: sync::Arc<T>) -> Result<(), sync::Arc<T>> {
        let slot = self
            .inner
            .strict_slot(affinity)
            .unwrap_or_else(|(index, slot_count)| out_of_coordinate_space(index, slot_count));
        slot.set(value)
    }

    /// Returns a clone of the value published for `affinity`'s strategy partition, if any.
    ///
    /// # Panics
    ///
    /// Panics if `affinity` falls outside the storage's coordinate space — an index at or beyond the
    /// partition count the strategy reports. Direct callers are expected to use affinities the
    /// storage was sized for.
    #[inline]
    #[must_use]
    pub fn get(&self, affinity: Affinity) -> Option<sync::Arc<T>> {
        let slot = self
            .inner
            .strict_slot(affinity)
            .unwrap_or_else(|(index, slot_count)| out_of_coordinate_space(index, slot_count));
        slot.get().cloned()
    }

    /// Returns a clone of the value published for `affinity`, or `None` when the affinity is out of
    /// range.
    ///
    /// The tolerant read the relocation hit path uses: an out-of-range affinity is a no-op there
    /// rather than a caller error, so it returns `None` instead of panicking the way the public
    /// [`get`](Self::get) does.
    pub(crate) fn probe(&self, affinity: Affinity) -> Option<sync::Arc<T>> {
        self.inner.get_clone(affinity)
    }

    /// Returns the write-once cell for `affinity`, or `None` when that affinity's slot is out of
    /// range.
    ///
    /// The caller drives the miss path with this: publish the materialized value into the empty
    /// cell. A cell is written at most once, so no lock is needed and no other thread can overwrite
    /// what this one publishes.
    pub(crate) fn slot(&self, affinity: Affinity) -> Option<&OnceLock<sync::Arc<T>>> {
        self.inner.slot(affinity)
    }

    /// Counts published values for which `predicate` holds.
    pub(crate) fn count_where(&self, predicate: impl Fn(&sync::Arc<T>) -> bool) -> usize {
        self.inner.count_where(predicate)
    }
}

impl<T: ?Sized, S: Strategy> Default for Storage<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

/// One slot: a write-once cell published on first materialization for its strategy partition.
type Slot<T> = OnceLock<T>;

/// Strategy-partitioned storage: one independently published slot per strategy partition.
///
/// This is the raw slot table behind [`Storage`], which fixes its element type to `Arc<T>`; the two
/// are separate so this can be unit-tested with a plain value type while the wrapper pins the stored
/// type.
///
/// Each slot is a [`OnceLock`], written at most once — on the first relocation
/// that materializes its strategy partition — and read by every later relocation.
/// The strategy decides how affinities map to slots: `PerCore` gives each processor
/// its own slot, while `PerNuma` and `PerProcess` map several affinities onto one
/// shared slot. A published read is a plain acquire load with no lock word to
/// contend on, so concurrent readers of distinct slots never serialize and
/// neighboring slots do not bounce a lock cache line between cores.
///
/// The slot array is sized once, on first use, to `S::count(affinity)` — a value
/// fixed for the process lifetime — so there is no growth path and therefore no
/// table-wide lock guarding it. After initialization the array and the pointer to
/// it are immutable, so reaching a slot is a plain atomic load that carries no
/// further synchronization; its cache behavior is left to the hardware.
#[derive(Debug)]
pub(crate) struct SlotTable<T, S: Strategy> {
    slots: OnceLock<Box<[Slot<T>]>>,
    _marker: PhantomData<S>,
}

impl<T, S: Strategy> SlotTable<T, S> {
    /// Creates a new empty `SlotTable` instance.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            slots: OnceLock::new(),
            _marker: PhantomData,
        }
    }

    /// Returns the slot array, sizing it on first use to the provided count.
    fn slots_with_count(&self, slot_count: usize) -> &[Slot<T>] {
        self.slots
            .get_or_init(|| (0..slot_count).map(|_| OnceLock::new()).collect::<Vec<_>>().into_boxed_slice())
    }

    /// Returns the slot array, sizing it on first use to hold every strategy partition.
    fn slots(&self, affinity: Affinity) -> &[Slot<T>] {
        if let Some(slots) = self.slots.get() {
            return slots;
        }

        self.slots_with_count(S::count(affinity).get())
    }

    /// Returns the write-once cell for `affinity`, or `None` when that affinity's index falls
    /// outside the sized table.
    ///
    /// The table is sized once, on first use, to the slot count the first affinity reported, so an
    /// index past its end means this affinity does not share that coordinate space. Rather than
    /// reaching into an unrelated slot, the lookup reports the slot as absent and leaves each caller
    /// to treat the affinity as unreachable.
    pub(crate) fn slot(&self, affinity: Affinity) -> Option<&Slot<T>> {
        self.slots(affinity).get(S::index(affinity))
    }

    /// Returns the slot for strict caller-facing access.
    ///
    /// An affinity that is already out of range for its own reported coordinate space is rejected
    /// before the table is initialized. A potentially valid affinity initializes the table before
    /// this returns, then is checked again against the effective slot count in case another
    /// coordinate space won the initialization race.
    pub(crate) fn strict_slot(&self, affinity: Affinity) -> Result<&Slot<T>, (usize, usize)> {
        let index = S::index(affinity);

        if let Some(slots) = self.slots.get() {
            return slots.get(index).ok_or((index, slots.len()));
        }

        let slot_count = S::count(affinity).get();
        if index >= slot_count {
            return Err((index, slot_count));
        }

        let slots = self.slots_with_count(slot_count);
        slots.get(index).ok_or((index, slots.len()))
    }

    /// Publishes `value` into the write-once cell for the given affinity if it is still empty.
    ///
    /// Returns `Ok(())` when the cell was empty and now holds `value`. When another publisher already
    /// filled it, or the affinity is out of range, the value cannot be stored and is handed back as
    /// `Err(value)`, mirroring [`OnceLock::set`](std::sync::OnceLock::set).
    #[cfg(test)]
    pub(crate) fn set_once(&self, affinity: Affinity, value: T) -> Result<(), T> {
        let Some(slot) = self.slot(affinity) else {
            return Err(value);
        };

        slot.set(value)
    }
}

std::thread_local! {
    /// Per-thread handle for the documented out-of-coordinate-space diagnostic event.
    static ARC_OOB: Event = Event::builder().name("thread_aware_arc_oob").build();
}

/// Records that a relocation reached an affinity whose slot index falls outside the sized table.
///
/// The table is sized once to one affinity's slot count, so an index past its end means the affinity
/// does not share that coordinate space and has no slot of its own. Relocation treats such an
/// affinity as unreachable and leaves the `Arc` on the value it already carries rather than reaching
/// into an unrelated slot. This is a supported-but-degraded path, so it is recorded as an observable
/// metric rather than trapped: a process that suspects it is happening can inspect the
/// `thread_aware_arc_oob` event. Direct `Storage` access never reaches here — it rejects an
/// out-of-range affinity by panicking instead. Ref: docs/implementation.md, "Storage".
///
/// Marked `#[cold]` and split into its own function so the emission machinery is laid out off the hot
/// path.
#[cold]
pub(crate) fn report_out_of_range_affinity() {
    ARC_OOB.with(Event::observe_once);
}

impl<T, S: Strategy> Default for SlotTable<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, S: Strategy> SlotTable<T, S>
where
    T: Clone,
{
    /// Clone and gets the data for the given affinity if it exists.
    /// Returns `None` if the data does not exist for that affinity.
    #[must_use]
    pub(crate) fn get_clone(&self, affinity: Affinity) -> Option<T> {
        self.slot(affinity)?.get().cloned()
    }

    /// Counts how many stored entries satisfy the given predicate.
    ///
    /// The count is an estimate under concurrent relocation, matching the
    /// inherently racy nature of a strong-count query.
    pub(crate) fn count_where(&self, predicate: impl Fn(&T) -> bool) -> usize {
        let Some(slots) = self.slots.get() else {
            return 0;
        };

        // Read each published value with a plain acquire load and apply the predicate by reference.
        // Slots are visited one at a time rather than under a single consistent snapshot, which is
        // what leaves the count an estimate.
        slots.iter().filter_map(|slot| slot.get()).filter(|value| predicate(value)).count()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::num::NonZero;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::affinity::pinned_affinities;
    use crate::storage::{SlotTable, Storage, Strategy};
    use crate::{PerCore, PerNuma, PerProcess};

    #[test]
    fn set_once_stores_then_rejects() {
        let affinities = pinned_affinities(&[1]);
        let storage = SlotTable::<String, PerCore>::default();
        let affinity = affinities[0];

        // The first publish into the empty cell succeeds.
        let result = storage.set_once(affinity, "First".to_string());
        assert_eq!(result, Ok(()));

        // A cell is written at most once, so later publishes are rejected and hand the value back
        // unchanged; the cell keeps its original contents.
        let result = storage.set_once(affinity, "Second".to_string());
        assert_eq!(result, Err("Second".to_string()));

        assert_eq!(storage.get_clone(affinity), Some("First".to_string()));
    }

    #[test]
    fn get_clone() {
        let affinities = pinned_affinities(&[1]);

        let storage = SlotTable::<String, PerCore>::default();
        let affinity = affinities[0];

        assert!(storage.get_clone(affinity).is_none());

        storage.set_once(affinity, "Hello".to_string()).unwrap();
        assert_eq!(storage.get_clone(affinity), Some("Hello".to_string()));
    }

    /// A `Strategy` that hands out an index outside the slot count it reports. Exists only to drive
    /// the out-of-range path in `SlotTable` and `Storage`.
    struct InconsistentStrategy;

    impl Strategy for InconsistentStrategy {
        fn index(_affinity: crate::affinity::Affinity) -> usize {
            1
        }

        fn count(_affinity: crate::affinity::Affinity) -> NonZero<usize> {
            NonZero::<usize>::MIN
        }
    }

    /// A strategy that records how often the slot count is requested.
    struct CountingStrategy;

    static COUNT_CALLS: AtomicUsize = AtomicUsize::new(0);

    impl Strategy for CountingStrategy {
        fn index(_affinity: crate::affinity::Affinity) -> usize {
            0
        }

        fn count(_affinity: crate::affinity::Affinity) -> NonZero<usize> {
            COUNT_CALLS.fetch_add(1, Ordering::AcqRel);
            NonZero::<usize>::MIN
        }
    }

    #[test]
    fn initialized_slot_table_does_not_recount_slots() {
        let affinity = pinned_affinities(&[1])[0];
        let table = SlotTable::<i32, CountingStrategy>::new();
        COUNT_CALLS.store(0, Ordering::Release);

        assert!(table.slot(affinity).is_some());
        assert!(table.slot(affinity).is_some());
        assert_eq!(
            COUNT_CALLS.load(Ordering::Acquire),
            1,
            "only table initialization may request the strategy's slot count"
        );
    }

    #[test]
    fn out_of_range_affinity_is_a_no_op() {
        let affinity = pinned_affinities(&[1])[0];
        let table = SlotTable::<i32, InconsistentStrategy>::new();

        // `index` reports 1 for a table sized to a single slot, so the requested slot is out of
        // range. The affinity has no cell of its own, so a publish through it stores nothing and
        // hands the value back, and a read finds nothing rather than reaching into an unrelated slot.
        assert_eq!(table.set_once(affinity, 42), Err(42));
        assert_eq!(table.get_clone(affinity), None);
    }

    #[test]
    fn out_of_range_report_records_oob_metric() {
        use nm::Report;

        fn oob_count() -> u64 {
            Report::collect()
                .events()
                .find(|event| event.name().as_ref() == "thread_aware_arc_oob")
                .map_or(0, nm::EventMetrics::count)
        }

        // Relocation records this metric when it reaches an out-of-range affinity. The registry is
        // process-wide, so the count is asserted as a strict increase rather than an absolute value.
        let before = oob_count();
        super::report_out_of_range_affinity();
        let after = oob_count();

        assert!(
            after > before,
            "the out-of-range report must record the thread_aware_arc_oob metric (before={before}, after={after})"
        );
    }

    #[test]
    #[should_panic(expected = "index 1 is outside its coordinate space (partition count: 1)")]
    fn insert_out_of_range_panics() {
        let affinity = pinned_affinities(&[1])[0];
        let storage = Storage::<i32, InconsistentStrategy>::new();

        // `InconsistentStrategy` indexes past its single-slot table, so a direct insert crosses the
        // coordinate space and is a caller error rather than the degraded relocation path.
        let _ = storage.insert(affinity, std::sync::Arc::new(1));
    }

    #[test]
    #[should_panic(expected = "index 1 is outside its coordinate space (partition count: 1)")]
    fn get_out_of_range_panics() {
        let affinity = pinned_affinities(&[1])[0];
        let storage = Storage::<i32, InconsistentStrategy>::new();

        let _ = storage.get(affinity);
    }

    #[test]
    fn out_of_range_storage_access_does_not_initialize_the_table() {
        let affinity = pinned_affinities(&[1])[0];
        let storage = Storage::<i32, InconsistentStrategy>::new();

        // The strategy's own count already proves the index invalid, so strict access can reject it
        // without allocating a table that no successful operation can use.
        let panic = std::panic::catch_unwind(|| storage.get(affinity));
        assert!(panic.is_err(), "strict out-of-range access must panic");
        assert!(
            storage.inner.slots.get().is_none(),
            "an immediately invalid access must leave the slot table uninitialized"
        );
    }

    #[test]
    fn probe_returns_published_value() {
        let affinity = pinned_affinities(&[1])[0];
        let storage = Storage::<i32, PerCore>::new();

        // An empty affinity probes as absent; after publishing, the probe returns that value. This
        // pins the hit-path fast read to its published slot: were `probe` to always report absent,
        // the relocation hit path would silently fall through to the slower write path and this
        // assertion would fail.
        assert!(storage.probe(affinity).is_none());

        let _ = storage.insert(affinity, std::sync::Arc::new(7));

        let probed = storage.probe(affinity).expect("probe returns the value just published");
        assert_eq!(*probed, 7);
    }

    #[test]
    fn per_app() {
        let affinities = pinned_affinities(&[1, 1]);

        let index = PerProcess::index(affinities[0]);
        let count = PerProcess::count(affinities[0]);
        assert_eq!(index, 0);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn per_memory_region() {
        let affinities = pinned_affinities(&[1, 1]);

        for affinity in affinities {
            let index = PerNuma::index(affinity);
            let count = PerNuma::count(affinity);
            assert_eq!(index, affinity.memory_region_index());
            assert_eq!(count.get(), affinity.memory_region_count());
        }
    }

    #[test]
    fn per_processor() {
        let affinities = pinned_affinities(&[1, 1]);

        for affinity in affinities {
            let index = PerCore::index(affinity);
            let count = PerCore::count(affinity);
            assert_eq!(index, affinity.processor_index());
            assert_eq!(count.get(), affinity.processor_count());
        }
    }

    #[test]
    fn test_default_implementation() {
        // This test covers line 101: Self::new() in the Default trait implementation
        let affinities = pinned_affinities(&[1]);

        // Create storage using Default trait - this exercises line 101
        let storage = SlotTable::<String, PerCore>::default();
        let affinity = affinities[0];

        // Verify the default storage is empty (no data for any affinity)
        assert!(storage.get_clone(affinity).is_none());

        // Verify it works the same as SlotTable::new()
        storage.set_once(affinity, "test".to_string()).unwrap();
        assert_eq!(storage.get_clone(affinity), Some("test".to_string()));
    }
}
