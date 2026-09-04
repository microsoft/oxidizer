// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Heap descriptors.

use std::cell::OnceCell;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HEAP_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static THREAD_HEAP: OnceCell<Heap> = const { OnceCell::new() };
    static THREAD_ID: ThreadId = {
        let id = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id, 0, "allocation hint thread identity space exhausted");
        ThreadId(id)
    };
}

/// General-purpose heap configuration.
pub mod general {
    const MEDIUM_SLICE_BYTES: usize = 64 * 1024;
    const MAX_LOCALITY_SEGMENT_BYTES: usize = 1024 * 1024 * 1024;
    const MAX_MEDIUM_CACHE_BYTES: usize = 8 * 1024 * 1024;

    /// Advisory options for a general-purpose heap.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Options {
        locality_segment_bytes: usize,
        medium_cache_max_bytes: usize,
    }

    impl Options {
        /// Returns the standard general-purpose heap options.
        #[must_use]
        pub const fn new() -> Self {
            Self {
                locality_segment_bytes: 4 * 1024 * 1024,
                medium_cache_max_bytes: MAX_MEDIUM_CACHE_BYTES,
            }
        }

        /// Sets the preferred locality segment size.
        ///
        /// # Panics
        ///
        /// Panics unless `bytes` is a power of two from 64 KiB through 1 GiB.
        #[must_use]
        pub const fn with_locality_segment_bytes(mut self, bytes: usize) -> Self {
            assert!(
                bytes >= MEDIUM_SLICE_BYTES && bytes <= MAX_LOCALITY_SEGMENT_BYTES && bytes.is_power_of_two(),
                "locality segment bytes must be a power of two from 64 KiB through 1 GiB"
            );
            self.locality_segment_bytes = bytes;
            self
        }

        /// Sets the largest preferred locally cached medium span.
        ///
        /// # Panics
        ///
        /// Panics unless `bytes` is zero or a power of two from 64 KiB through 8 MiB.
        #[must_use]
        pub const fn with_medium_cache_max_bytes(mut self, bytes: usize) -> Self {
            assert!(
                bytes == 0 || (bytes >= MEDIUM_SLICE_BYTES && bytes <= MAX_MEDIUM_CACHE_BYTES && bytes.is_power_of_two()),
                "medium cache maximum bytes must be zero or a power of two from 64 KiB through 8 MiB"
            );
            self.medium_cache_max_bytes = bytes;
            self
        }

        /// Returns the preferred locality segment size.
        #[must_use]
        pub const fn locality_segment_bytes(self) -> usize {
            self.locality_segment_bytes
        }

        /// Returns the largest preferred locally cached medium span.
        #[must_use]
        pub const fn medium_cache_max_bytes(self) -> usize {
            self.medium_cache_max_bytes
        }
    }

    impl Default for Options {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// Bump heap configuration.
pub mod bump {
    const BUMP_SEGMENT_SIZE: usize = 32 * 1024;

    /// Advisory options for a bump heap.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Options {
        max_allocation_bytes: usize,
        max_alignment: usize,
        retained_chunks: usize,
        max_retained_chunks: usize,
    }

    impl Options {
        /// Returns the standard bump heap options.
        #[must_use]
        pub const fn new() -> Self {
            Self {
                max_allocation_bytes: BUMP_SEGMENT_SIZE,
                max_alignment: 4 * 1024,
                retained_chunks: 4,
                max_retained_chunks: 16,
            }
        }

        /// Sets the largest allocation eligible for bump allocation.
        ///
        /// # Panics
        ///
        /// Panics if `bytes` is zero or exceeds 32 KiB.
        #[must_use]
        pub const fn with_max_allocation_bytes(mut self, bytes: usize) -> Self {
            assert!(
                bytes != 0 && bytes <= BUMP_SEGMENT_SIZE,
                "bump maximum allocation bytes must be from 1 byte through 32 KiB"
            );
            self.max_allocation_bytes = bytes;
            self
        }

        /// Sets the largest allocation alignment eligible for bump allocation.
        ///
        /// # Panics
        ///
        /// Panics unless `alignment` is a nonzero power of two through 32 KiB.
        #[must_use]
        pub const fn with_max_alignment(mut self, alignment: usize) -> Self {
            assert!(
                alignment != 0 && alignment <= BUMP_SEGMENT_SIZE && alignment.is_power_of_two(),
                "bump maximum alignment must be a power of two through 32 KiB"
            );
            self.max_alignment = alignment;
            self
        }

        /// Sets the minimum chunks retained by a supporting allocator's cache.
        ///
        /// # Panics
        ///
        /// Panics if `chunks` is zero.
        #[must_use]
        pub const fn with_retained_chunks(mut self, chunks: usize) -> Self {
            assert!(chunks != 0, "a bump heap must retain at least its root chunk");
            self.retained_chunks = chunks;
            self.max_retained_chunks = chunks;
            self
        }

        /// Sets the maximum chunks retained by a supporting allocator's cache.
        ///
        /// # Panics
        ///
        /// Panics if `chunks` is below the retained minimum.
        #[must_use]
        pub const fn with_max_retained_chunks(mut self, chunks: usize) -> Self {
            assert!(
                chunks >= self.retained_chunks,
                "maximum retained chunks must not be below the retained minimum"
            );
            self.max_retained_chunks = chunks;
            self
        }

        /// Returns the largest bump-eligible allocation.
        #[must_use]
        pub const fn max_allocation_bytes(self) -> usize {
            self.max_allocation_bytes
        }

        /// Returns the largest bump-eligible alignment.
        #[must_use]
        pub const fn max_alignment(self) -> usize {
            self.max_alignment
        }

        /// Returns the minimum retained chunk count.
        #[must_use]
        pub const fn retained_chunks(self) -> usize {
            self.retained_chunks
        }

        /// Returns the maximum retained chunk count.
        #[must_use]
        pub const fn max_retained_chunks(self) -> usize {
            self.max_retained_chunks
        }
    }

    impl Default for Options {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// The requested logical heap kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// A general-purpose heap.
    General(general::Options),
    /// A bump heap.
    Bump(bump::Options),
    /// The allocator-preferred heap belonging to a particular thread.
    Thread(ThreadId),
}

/// A process-unique logical heap identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HeapId(u64);

impl HeapId {
    /// Returns the numeric identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A process-unique logical thread identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadId(u64);

impl ThreadId {
    /// Returns the numeric identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub(crate) struct Descriptor {
    pub(crate) id: HeapId,
    pub(crate) kind: Kind,
}

/// A prospective logical heap handle.
///
/// Creating or dropping this handle does not call an allocator. A supporting
/// allocator realizes its descriptor only while observing it as the active
/// hint during an allocation.
#[derive(Clone)]
pub struct Heap {
    pub(crate) descriptor: Arc<Descriptor>,
}

impl Heap {
    /// Creates a prospective general-purpose heap.
    #[must_use]
    pub fn new() -> Self {
        Self::general(general::Options::new())
    }

    /// Creates a prospective general-purpose heap with `options`.
    #[must_use]
    pub fn general(options: general::Options) -> Self {
        Self::from_kind(Kind::General(options))
    }

    /// Creates a prospective bump heap.
    #[must_use]
    pub fn bump(options: bump::Options) -> Self {
        Self::from_kind(Kind::Bump(options))
    }

    /// Returns this logical heap's identity.
    #[must_use]
    pub fn id(&self) -> HeapId {
        self.descriptor.id
    }

    /// Returns this logical heap's requested kind.
    #[must_use]
    pub fn kind(&self) -> Kind {
        self.descriptor.kind
    }

    fn from_kind(kind: Kind) -> Self {
        let id = NEXT_HEAP_ID.fetch_add(1, Ordering::Relaxed);
        assert_ne!(id, 0, "allocation hint heap identity space exhausted");
        Self {
            descriptor: Arc::new(Descriptor { id: HeapId(id), kind }),
        }
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Heap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Heap")
            .field("id", &self.descriptor.id)
            .field("kind", &self.descriptor.kind)
            .finish()
    }
}

/// A copy of the current thread's requested allocation hint.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveHint {
    id: HeapId,
    kind: Kind,
}

impl ActiveHint {
    pub(crate) const fn new(id: HeapId, kind: Kind) -> Self {
        Self { id, kind }
    }

    /// Returns the logical heap identity.
    #[must_use]
    pub const fn id(self) -> HeapId {
        self.id
    }

    /// Returns the requested heap kind.
    #[must_use]
    pub const fn kind(self) -> Kind {
        self.kind
    }
}

/// Returns a logical handle for the current thread's allocator-preferred heap.
///
/// Supporting allocators resolve this identity to the heap they use for
/// ordinary allocations on this thread. The handle can then be sent to another
/// thread and installed with [`crate::with_hint`]. Other allocators may ignore it.
#[must_use]
pub fn thread_heap() -> Heap {
    let thread_id = current_thread_id();
    crate::request_thread_heap(thread_id);
    THREAD_HEAP.with(|heap| heap.get_or_init(|| Heap::from_kind(Kind::Thread(thread_id))).clone())
}

/// Returns the current thread's logical identity.
#[doc(hidden)]
#[must_use]
pub fn current_thread_id() -> ThreadId {
    THREAD_ID.with(|id| *id)
}

/// Returns the thread identity currently requesting a thread-heap handle.
#[doc(hidden)]
#[must_use]
pub fn thread_heap_request() -> Option<ThreadId> {
    crate::allocator_hints().thread_heap()
}
