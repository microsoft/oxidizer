// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native v4 owner observations, separate from application allocation events.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static PUBLICATION_ENABLED: AtomicBool = AtomicBool::new(true);
static OBSERVATION_ROUND: AtomicU64 = AtomicU64::new(1);
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Whether accepted recorded allocation/free operations may publish observations.
///
/// Sampled-out operations and merely enabled recording attempts do not publish.
#[must_use]
pub fn publication_enabled() -> bool {
    PUBLICATION_ENABLED.load(Ordering::Relaxed)
}

/// Enables or disables publication; this does not enable event recording.
pub fn set_publication_enabled(enabled: bool) {
    PUBLICATION_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Round requested by the most recent collector or explicit app-side request.
#[must_use]
pub fn observation_round() -> u64 {
    OBSERVATION_ROUND.load(Ordering::Relaxed)
}

/// Requests another observation on the next accepted recorded allocation/free.
///
/// Collectors request the next round only after collection. Monitor polling
/// drives successive rounds; applications can also schedule requests explicitly.
/// No timer or background worker is started. Saturates instead of wrapping.
pub fn request_observation() -> u64 {
    let previous = OBSERVATION_ROUND.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |round| Some(round.saturating_add(1)));
    match previous {
        Ok(round) | Err(round) => round.saturating_add(1),
    }
}

/// Process-relative monotonic time, saturated to the wire representation.
#[must_use]
pub fn captured_nanos() -> u64 {
    u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Number of native small-object size classes.
pub const CLASS_COUNT: usize = 44;
/// Number of power-of-two range bins.
pub const RANGE_COUNT: usize = 64;

/// An owner-exclusive class summary, not an application-live allocation count.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClassState {
    /// Rounded object size.
    pub object_bytes: u64,
    /// Backing size of one slab.
    pub slab_bytes: u64,
    /// Object capacity of one slab.
    pub capacity: u64,
    /// Slabs in the native available list.
    pub available_slabs: u64,
    /// Empty reusable slabs tracked by the native allocator.
    pub empty_slabs: u64,
    /// Slabs found in bounded available and laden list walks.
    pub observed_slabs: u64,
    /// Whether the allocation-ready fast list has an object.
    pub fast_nonempty: bool,
}

/// Counts of owned ranges, indexed by their base-two size exponent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ranges {
    /// Bin `n` contains ranges of `1 << n` bytes.
    pub counts: [u64; RANGE_COUNT],
    /// False when the bounded walk omitted nodes.
    pub complete: bool,
}

impl Ranges {
    /// An empty, complete range inventory.
    pub const EMPTY: Self = Self {
        counts: [0; RANGE_COUNT],
        complete: true,
    };
}

impl Default for Ranges {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Owner-local backend caches and native growth state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalState {
    /// Committed reusable object ranges.
    pub ranges: Ranges,
    /// Reusable metadata ranges.
    pub metadata: Ranges,
    /// Cumulative native local-refill growth state, not retained bytes.
    pub requested_bytes: u64,
}

/// Remote-return state; queued returns are not application-live objects.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RemoteState {
    /// Nonempty outgoing open rings.
    pub open_rings: u64,
    /// Objects still held in outgoing open rings.
    pub open_objects: u64,
    /// Nonempty outgoing message buckets.
    pub outgoing_lists: u64,
    /// Messages found in bounded outgoing-list walks.
    pub messages: u64,
    /// Objects in those messages.
    pub message_objects: u64,
    /// Class-rounded bytes in those messages.
    pub message_bytes: u64,
    /// False when outgoing-list walks omitted nodes.
    pub complete: bool,
    /// Remaining native batching budget, not queued bytes.
    pub budget_remaining: u64,
    /// Atomic incoming front address; never dereferenced by the collector.
    pub incoming_front: u64,
    /// Atomic incoming back address; never dereferenced by the collector.
    pub incoming_back: u64,
}

/// A bounded observation copied under an existing native owner lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    /// Lease generation to which this observation belongs.
    pub generation: u64,
    /// Recorder session accepting the contributing operation; zero for idle inspection.
    pub session_id: u64,
    /// Observation round.
    pub round: u64,
    /// Contributing recorder thread; zero when inspected by the collector.
    pub thread_id: u64,
    /// Process-relative monotonic capture time in nanoseconds.
    pub captured_nanos: u64,
    /// False when the slab inventory walk was truncated.
    pub slabs_complete: bool,
    /// Small-object class state.
    pub classes: [ClassState; CLASS_COUNT],
    /// Outstanding allocator ranges, including pending/retained frees, not app-live objects.
    pub large: Ranges,
    /// Owner-local backend state.
    pub local: LocalState,
    /// Outgoing and incoming remote-return state.
    pub remote: RemoteState,
}

impl Observation {
    /// An empty observation to populate under an owner lease.
    pub const EMPTY: Self = Self {
        generation: 0,
        session_id: 0,
        round: 0,
        thread_id: 0,
        captured_nanos: 0,
        slabs_complete: true,
        classes: [ClassState {
            object_bytes: 0,
            slab_bytes: 0,
            capacity: 0,
            available_slabs: 0,
            empty_slabs: 0,
            observed_slabs: 0,
            fast_nonempty: false,
        }; CLASS_COUNT],
        large: Ranges::EMPTY,
        local: LocalState {
            ranges: Ranges::EMPTY,
            metadata: Ranges::EMPTY,
            requested_bytes: 0,
        },
        remote: RemoteState {
            open_rings: 0,
            open_objects: 0,
            outgoing_lists: 0,
            messages: 0,
            message_objects: 0,
            message_bytes: 0,
            complete: true,
            budget_remaining: 0,
            incoming_front: 0,
            incoming_back: 0,
        },
    };
}

impl Default for Observation {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// How an owner row obtained its state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObservationSource {
    /// No safely readable observation exists.
    #[default]
    Unobserved,
    /// A thread published after an accepted recorded allocation/free completed.
    Published,
    /// The collector inspected a returned owner under the pool lock.
    IdleInspection,
    /// The published slot was busy during bounded collection.
    Busy,
    /// System allocation of the publication slot failed; owner state is unknown.
    Unavailable,
}

/// A persistent native endpoint, including endpoints that never recorded events.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Owner {
    /// Persistent endpoint address, not an allocation lifetime identity.
    pub id: u64,
    /// Current lease generation; native producer uses odd for leased, even for returned.
    pub generation: u64,
    /// Whether a thread currently holds this endpoint's lease.
    pub leased: bool,
    /// Origin of the observation.
    pub source: ObservationSource,
    /// Last readable observation, possibly from an older session or lease.
    pub observation: Option<Observation>,
}

/// Lock-protected global backend inventory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GlobalState {
    /// Cumulative successful native object-range reservations, excluding the pagemap.
    pub reserved_bytes: u64,
    /// Global cached ranges; discard failures mean physical residency is unknown.
    pub ranges: Ranges,
    /// Virtual address space reserved for the sparse pagemap.
    pub pagemap_reserved_bytes: u64,
    /// Native local growth/range-cache boundary.
    pub local_limit_bytes: u64,
    /// Native global refill-growth ceiling; individual larger requests are possible.
    pub global_refill_bytes: u64,
}

/// Native v4 state payload; allocation events remain in the Seismograph container.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    /// Process-relative monotonic collection time.
    pub captured_nanos: u64,
    /// Recorder session represented by the surrounding container.
    pub session_id: u64,
    /// Round requested before this collection.
    pub round: u64,
    /// Total endpoints in the native inventory at collection.
    pub owner_count: u64,
    /// False when the bounded owner inventory omitted endpoints.
    pub owners_complete: bool,
    /// Whether self-publication is enabled.
    pub publication_enabled: bool,
    /// All captured endpoint rows, including unknown and stale observations.
    pub owners: Vec<Owner>,
    /// Independently collected global backend state.
    pub global: GlobalState,
}

/// Freshness of readable state, separate from whether the slot was busy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Freshness {
    /// No safely readable observation exists.
    Unknown,
    /// Independently inspected idle owner in this capture.
    IdleInspection,
    /// Same session, lease and requested round: contributed this round, not a current census.
    Current,
    /// Publication belongs to a prior lease; contributor is not the current actor.
    PreviousLease,
    /// Publication belongs to another recording session.
    PreviousSession,
    /// Same lease/session but from an older requested round.
    PreviousRound,
    /// Publication is newer than the capture's round or monotonic time.
    NewerThanCapture,
}

impl Freshness {
    /// Concise coverage label for reports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown / unobserved",
            Self::IdleInspection => "fresh idle inspection",
            Self::Current => "contributed this round",
            Self::PreviousLease => "stale lease",
            Self::PreviousSession => "stale session",
            Self::PreviousRound => "older round",
            Self::NewerThanCapture => "newer than capture",
        }
    }
}

impl Owner {
    /// Classifies observation provenance without treating missing state as zero.
    #[must_use]
    pub fn freshness(&self, snapshot: &Snapshot) -> Freshness {
        let Some(observation) = &self.observation else {
            return Freshness::Unknown;
        };
        if observation.generation != self.generation {
            return Freshness::PreviousLease;
        }
        if self.source == ObservationSource::IdleInspection {
            return Freshness::IdleInspection;
        }
        if observation.session_id == 0 || observation.session_id != snapshot.session_id {
            return Freshness::PreviousSession;
        }
        if observation.round > snapshot.round || observation.captured_nanos > snapshot.captured_nanos {
            return Freshness::NewerThanCapture;
        }
        if observation.round < snapshot.round {
            return Freshness::PreviousRound;
        }
        Freshness::Current
    }
}

impl Ranges {
    /// Class-rounded native range capacity, never application-live bytes.
    ///
    /// Incomplete walks yield only the observed capacity.
    #[must_use]
    pub fn observed_bytes(&self) -> u128 {
        self.counts
            .iter()
            .enumerate()
            .map(|(exponent, count)| u128::from(*count) << exponent)
            .sum()
    }
}
