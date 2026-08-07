// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::inline_always,
    reason = "Scoped hint entry and restoration are allocator hot paths intentionally forced inline"
)]
#![expect(
    clippy::missing_errors_doc,
    reason = "Heap operations return the crate's small documented error type"
)]
#![expect(
    clippy::missing_panics_doc,
    reason = "Infallible constructors and validated builders panic only on documented contract violations"
)]
#![expect(clippy::panic, reason = "Infallible public APIs and invariant checks intentionally panic")]
#![expect(
    clippy::renamed_function_params,
    reason = "Implementation parameter names are clearer than generic trait names"
)]
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "Unsafe backend calls are covered by the Backend and raw target safety contracts"
)]
#![cfg_attr(
    test,
    expect(
        clippy::items_after_statements,
        clippy::multiple_unsafe_ops_per_block,
        clippy::unnecessary_wraps,
        reason = "Test callbacks intentionally mirror the backend ABI and keep setup local to each scenario"
    )
)]

//! Infrastructure for allocator-agnostic heap allocation hints.
//!
//! Libraries can use [`with_hint`] to set a thread-local [`Hint`] that a
//! supporting global allocator can query during allocation.
//!
//! This lets users direct the allocator to prefer a particular heap or allocation
//! strategy, potentially improving performance and locality while reducing long-term
//! fragmentation.
//!
//! The underlying model is structured into domains, heaps, and hints. A **domain** is a set of
//! equal-sized, low-level "dumb" allocation regions obtained from the operating system. Allocators
//! internally divide these into subregions that heaps can reserve and return. A **heap** then
//! manages its assigned subregions according to its preferred strategy. One type of heap might
//! optimize for fixed size classes, another might treat its regions as bump space, and yet another
//! might act as a _general-purpose_ heap using mixed-mode allocation.
//!
//! At the lowest level, **hints** are ad hoc instructions to the current thread's allocator to
//! prefer a particular heap. However, hints always remain advisory. Allocators that do not support
//! them may ignore them, while preserving all existing allocation API contracts.
//!
//! # Example
//!
//! This requires a supporting global allocator backend to be registered first.
//!
//! ```no_run
//! use allocation_hints::heap::{Heap, bump};
//! use allocation_hints::with_hint;
//!
//! let heap = Heap::bump(bump::Options::new());
//! with_hint(&heap, || {
//!     let values = vec![1, 2, 3];
//!     assert_eq!(values.len(), 3);
//! });
//! ```

pub mod backend;
pub mod domain;
pub mod heap;

use std::cell::Cell;
use std::{fmt, ptr};

use backend::{Backend, ClaimPolicy, RawHint};
use heap::{Heap, HeapId};

const MAX_CLAIMED_HEAPS: usize = 16;

thread_local! {
    static CONTEXT: Cell<ThreadContext> = const { Cell::new(ThreadContext::new()) };
}

/// An error reported by an allocation-hint operation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
}

/// Stable category of an allocation-hint operation error.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// Heap inspection could not proceed while another thread was actively using the heap.
    InspectionContended,
    /// Usage information is unavailable from the calling thread.
    UsageUnavailable,
}

impl Error {
    /// Returns the stable category of this error.
    #[must_use]
    pub const fn kind(self) -> ErrorKind {
        self.kind
    }

    const fn inspection_contended() -> Self {
        Self {
            kind: ErrorKind::InspectionContended,
        }
    }

    const fn usage_unavailable() -> Self {
        Self {
            kind: ErrorKind::UsageUnavailable,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::InspectionContended => formatter.write_str("the heap is active on another thread"),
            ErrorKind::UsageUnavailable => formatter.write_str("thread-heap usage must be queried from its owner thread"),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Error({self})")
    }
}

impl std::error::Error for Error {}

/// Allocator-independent options applied by [`with_hint`].
pub struct Hint {
    heap: Option<HeapId>,
    not_sync: std::marker::PhantomData<Cell<()>>,
}

impl Hint {
    /// Creates a hint selecting the process-global allocation target.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            heap: None,
            not_sync: std::marker::PhantomData,
        }
    }

    /// Selects the process-global allocation target.
    #[must_use]
    pub const fn global() -> Self {
        Self::new()
    }

    /// Routes allocations through [`heap::Heap`].
    #[must_use]
    pub fn with_heap(mut self, heap: &Heap) -> Self {
        self.heap = Some(heap.id.clone());
        self
    }

    fn raw_hint(&self) -> RawHint {
        self.heap.as_ref().map_or(RawHint::GLOBAL, HeapId::raw_hint)
    }
}

impl Clone for Hint {
    fn clone(&self) -> Self {
        Self {
            heap: self.heap.clone(),
            not_sync: std::marker::PhantomData,
        }
    }
}

impl Default for Hint {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&Heap> for Hint {
    fn from(heap: &Heap) -> Self {
        Self::new().with_heap(heap)
    }
}

impl PartialEq for Hint {
    fn eq(&self, other: &Self) -> bool {
        match (&self.heap, &other.heap) {
            (Some(left), Some(right)) => left.identity() == right.identity(),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for Hint {}

impl fmt::Debug for Hint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Hint")
            .field("heap", &self.heap.as_ref().map(HeapId::identity))
            .finish()
    }
}

/// Runs `operation` with an allocation hint active on the current thread.
///
/// Calls may be nested. The previous hint is restored even if `operation`
/// panics. A [`Hint`] or a borrowed [`heap::Heap`] may be passed directly.
/// At most 16 distinct exclusive heaps may be active in one nested scope chain;
/// re-entering an already-active heap does not consume another slot.
#[inline(always)]
pub fn with_hint<R>(hint: impl Into<Hint>, operation: impl FnOnce() -> R) -> R {
    let hint = hint.into();
    let snapshot = enter(&hint);
    let _restore = RestoreContext {
        snapshot: Some(snapshot),
        hint,
    };
    operation()
}

struct ContextSnapshot {
    previous: RawHint,
    claimed: bool,
}

struct RestoreContext {
    snapshot: Option<ContextSnapshot>,
    hint: Hint,
}

impl Drop for RestoreContext {
    #[inline(always)]
    fn drop(&mut self) {
        let snapshot = self.snapshot.take().expect("allocation hint restored more than once");
        CONTEXT.with(|context| {
            let mut current = context.get();
            if let Some(backend) = current.backend {
                unsafe { (backend.activate)(current.backend_context, snapshot.previous) };
            }
            current.active = snapshot.previous;
            if snapshot.claimed {
                let heap = self.hint.heap.as_ref().expect("claimed allocation hint must contain a heap");
                current.claimed_len -= 1;
                debug_assert_eq!(current.claimed[current.claimed_len], heap.identity());
                heap.release();
            }
            context.set(current);
        });
    }
}

struct PendingClaim<'a> {
    heap: Option<&'a HeapId>,
}

impl PendingClaim<'_> {
    fn commit(&mut self) {
        self.heap = None;
    }
}

impl Drop for PendingClaim<'_> {
    fn drop(&mut self) {
        if let Some(heap) = self.heap {
            heap.release();
        }
    }
}

#[inline(always)]
fn enter(hint: &Hint) -> ContextSnapshot {
    CONTEXT.with(|context| {
        let mut current = context.get();
        let claimed = if let Some(heap) = hint.heap.as_ref() {
            let identity = heap.identity();
            if heap.claim_policy() == ClaimPolicy::Shared || current.claimed[..current.claimed_len].contains(&identity) {
                false
            } else {
                assert!(current.claimed_len < current.claimed.len(), "too many nested active heaps");
                assert!(heap.claim(), "a heap cannot be active on multiple threads or scopes");
                true
            }
        } else {
            false
        };
        let mut pending_claim = PendingClaim {
            heap: claimed.then(|| hint.heap.as_ref().expect("claimed allocation hint must contain a heap")),
        };
        let previous = current.active;
        let active = hint.raw_hint();
        if let Some(heap) = hint.heap.as_ref()
            && current.backend.is_none()
        {
            let backend = heap.backend();
            let backend_context = (backend.thread_context)();
            assert!(!backend_context.is_null(), "allocator backend returned a null thread context");
            current.backend = Some(backend);
            current.backend_context = backend_context;
        }
        if let Some(backend) = current.backend {
            unsafe { (backend.activate)(current.backend_context, active) };
        }
        current.active = active;
        if claimed {
            let heap = hint.heap.as_ref().expect("claimed allocation hint must contain a heap");
            current.claimed[current.claimed_len] = heap.identity();
            current.claimed_len += 1;
        }
        context.set(current);
        pending_claim.commit();
        ContextSnapshot { previous, claimed }
    })
}

pub(crate) fn is_claimed(identity: usize) -> bool {
    CONTEXT.with(|context| {
        let context = context.get();
        context.claimed[..context.claimed_len].contains(&identity)
    })
}

#[derive(Clone, Copy)]
struct ThreadContext {
    active: RawHint,
    backend: Option<&'static Backend>,
    backend_context: *mut (),
    claimed: [usize; MAX_CLAIMED_HEAPS],
    claimed_len: usize,
}

impl ThreadContext {
    const fn new() -> Self {
        Self {
            active: RawHint::GLOBAL,
            backend: None,
            backend_context: ptr::null_mut(),
            claimed: [0; MAX_CLAIMED_HEAPS],
            claimed_len: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::backend::{RawDomain, RawHeap};
    use crate::domain::Domain;
    use crate::heap::{CreationError, Info, InfoKind, Options, Usage, UsageKind, bump, general};

    static_assertions::assert_impl_all!(Hint: Clone);
    static_assertions::assert_not_impl_any!(Hint: Sync);

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static ACTIVE_KIND: AtomicUsize = AtomicUsize::new(0);
    static NEXT_DOMAIN_IDENTITY: AtomicUsize = AtomicUsize::new(2);
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    static THREAD_CONTEXT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static PANIC_ON_ACTIVATE: AtomicBool = AtomicBool::new(false);

    fn create_domain() -> Option<RawDomain> {
        Some(unsafe { RawDomain::new(ptr::without_provenance_mut(NEXT_DOMAIN_IDENTITY.fetch_add(1, Ordering::Relaxed))) })
    }

    fn default_domain() -> RawDomain {
        unsafe { RawDomain::new(NonNull::<u8>::dangling().as_ptr().cast()) }
    }

    fn create(_: Options) -> Result<RawHeap, CreationError> {
        Ok(unsafe { RawHeap::new(RawHint::new(NonNull::<u8>::dangling().as_ptr().cast(), 3), ClaimPolicy::Exclusive) })
    }

    fn thread_heap() -> Option<RawHeap> {
        Some(unsafe { RawHeap::new(RawHint::new(NonNull::<u8>::dangling().as_ptr().cast(), 4), ClaimPolicy::Shared) })
    }

    fn thread_context() -> *mut () {
        THREAD_CONTEXT_CALLS.fetch_add(1, Ordering::Relaxed);
        NonNull::<u8>::dangling().as_ptr().cast()
    }

    unsafe fn activate(_: *mut (), hint: RawHint) {
        assert!(!PANIC_ON_ACTIVATE.swap(false, Ordering::Relaxed), "injected activation panic");
        ACTIVE_KIND.store(hint.kind(), Ordering::Relaxed);
    }

    unsafe fn info(_: RawHint, active: bool) -> Info {
        Info::new(
            active,
            unsafe { Domain::from_raw(RawDomain::new(NonNull::<u8>::dangling().as_ptr().cast()), &TEST_BACKEND) },
            InfoKind::General(general::Info::new(general::Options::new(), false)),
        )
    }

    unsafe fn usage(_: RawHint) -> Result<Usage, ()> {
        Ok(Usage::new(
            0,
            0,
            0,
            0,
            0,
            UsageKind::General(general::Usage::new(
                general::AllocationUsage::default(),
                general::AllocationUsage::default(),
                general::AllocationUsage::default(),
                0,
                0,
                0,
            )),
        ))
    }

    unsafe fn destroy(_: RawHint) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }

    static TEST_BACKEND: Backend = unsafe {
        Backend::new(
            create_domain,
            default_domain,
            create,
            thread_heap,
            thread_context,
            activate,
            info,
            usage,
            destroy,
        )
    };
    static OTHER_BACKEND: Backend = unsafe {
        Backend::new(
            create_domain,
            default_domain,
            create,
            thread_heap,
            thread_context,
            activate,
            info,
            usage,
            destroy,
        )
    };

    fn heap(kind: usize, claim_policy: ClaimPolicy) -> Heap {
        unsafe {
            Heap::from_raw(
                RawHint::new(NonNull::<u8>::dangling().as_ptr().cast(), kind),
                &TEST_BACKEND,
                claim_policy,
            )
        }
    }

    #[test]
    fn backend_factory_creates_common_heaps() {
        let _test = TEST_LOCK.lock().unwrap();
        unsafe { crate::backend::register(&TEST_BACKEND) };
        let heap = Heap::bump(bump::Options::new());
        let fallible_heap = Heap::try_bump(bump::Options::new()).unwrap();
        assert_ne!(heap.identity(), 0);
        assert_ne!(fallible_heap.identity(), 0);
        assert!(matches!(heap.info().kind(), InfoKind::General(_)));
        assert!(heap.usage().unwrap().is_empty());
    }

    #[test]
    fn backend_factory_creates_thread_heaps() {
        let _test = TEST_LOCK.lock().unwrap();
        unsafe { crate::backend::register(&TEST_BACKEND) };
        let heap = crate::heap::thread_heap().unwrap();
        assert_eq!(heap.id.raw_hint().kind(), 4);
        assert!(!heap.is_claimed());
    }

    #[test]
    fn backend_factory_creates_common_domains() {
        let _test = TEST_LOCK.lock().unwrap();
        unsafe { crate::backend::register(&TEST_BACKEND) };
        let first = Domain::new();
        let second = Domain::try_new().unwrap();
        let default = Domain::default();

        assert_ne!(first, second);
        assert_ne!(first, default);
        assert_eq!(default, Domain::default());
    }

    #[test]
    fn backend_registration_rejects_a_different_provider() {
        let _test = TEST_LOCK.lock().unwrap();
        unsafe { crate::backend::register(&TEST_BACKEND) };
        assert!(
            std::panic::catch_unwind(|| unsafe {
                crate::backend::register(&OTHER_BACKEND);
            })
            .is_err()
        );
    }

    #[test]
    fn nested_scopes_restore_targets_and_reuse_claims() {
        let _test = TEST_LOCK.lock().unwrap();
        let heap = heap(1, ClaimPolicy::Exclusive);
        assert!(!heap.is_claimed());
        with_hint(&heap, || {
            assert_eq!(ACTIVE_KIND.load(Ordering::Relaxed), 1);
            assert!(heap.is_claimed());
            with_hint(&heap, || {
                assert_eq!(ACTIVE_KIND.load(Ordering::Relaxed), 1);
            });
            assert_eq!(ACTIVE_KIND.load(Ordering::Relaxed), 1);
        });
        assert_eq!(ACTIVE_KIND.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn hints_retain_targets_and_restore_after_panics() {
        let _test = TEST_LOCK.lock().unwrap();
        let before = DROPS.load(Ordering::Relaxed);
        let heap = heap(2, ClaimPolicy::Shared);
        let hint = Hint::new().with_heap(&heap);
        drop(heap);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_hint(hint, || panic!("expected panic"));
        }));
        assert!(result.is_err());
        assert_eq!(ACTIVE_KIND.load(Ordering::Relaxed), 0);
        assert_eq!(DROPS.load(Ordering::Relaxed), before + 1);
    }

    #[test]
    fn failed_activation_releases_a_new_claim() {
        let _test = TEST_LOCK.lock().unwrap();
        let heap = heap(2, ClaimPolicy::Exclusive);
        PANIC_ON_ACTIVATE.store(true, Ordering::Relaxed);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_hint(Hint::new().with_heap(&heap), || {});
        }));

        assert!(result.is_err());
        assert!(!heap.is_claimed());
        assert_eq!(ACTIVE_KIND.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn native_thread_context_is_cached_per_thread() {
        let _test = TEST_LOCK.lock().unwrap();
        THREAD_CONTEXT_CALLS.store(0, Ordering::Relaxed);
        std::thread::spawn(|| {
            let heap = heap(1, ClaimPolicy::Exclusive);
            with_hint(Hint::new().with_heap(&heap), || {});
            with_hint(Hint::new().with_heap(&heap), || {});
        })
        .join()
        .unwrap();
        assert_eq!(THREAD_CONTEXT_CALLS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn nonblocking_inspection_reports_an_exclusive_claim_on_another_thread() {
        let _test = TEST_LOCK.lock().unwrap();
        let heap = heap(1, ClaimPolicy::Exclusive);
        let remote_hint = Hint::new().with_heap(&heap);
        let active = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let remote_active = Arc::clone(&active);
        let remote_release = Arc::clone(&release);
        let remote = std::thread::spawn(move || {
            with_hint(remote_hint, || {
                remote_active.wait();
                remote_release.wait();
            });
        });

        active.wait();
        assert_eq!(heap.try_info().unwrap_err().to_string(), "the heap is active on another thread");
        assert_eq!(heap.try_usage().unwrap_err().to_string(), "the heap is active on another thread");
        release.wait();
        remote.join().unwrap();
        heap.try_info().unwrap();
        heap.try_usage().unwrap();
    }

    #[test]
    fn exclusive_heap_nesting_is_bounded() {
        let _test = TEST_LOCK.lock().unwrap();
        let heaps = (0..=MAX_CLAIMED_HEAPS)
            .map(|index| heap(index + 1, ClaimPolicy::Exclusive))
            .collect::<Vec<_>>();

        fn enter_all(heaps: &[Heap]) {
            if let Some((heap, remaining)) = heaps.split_first() {
                with_hint(heap, || enter_all(remaining));
            }
        }

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| enter_all(&heaps)));
        assert!(result.is_err());
        assert!(heaps.iter().all(|heap| !heap.is_claimed()));
    }
}
