// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocator-independent heap ownership, configuration, and inspection.

pub mod bump;
pub mod general;

use std::cell::Cell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backend::{self, Backend, ClaimPolicy, RawHeap, RawHint};
use crate::domain::Domain;

/// The allocation strategy selected for a [`Heap`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Kind {
    /// A general-purpose heap.
    General(general::Options),
    /// A bump heap.
    Bump(bump::Options),
}

/// The backing-state creation policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CreationPolicy {
    /// Create fresh backing state.
    Fresh,
    /// Reuse empty backing state from the current thread or process pool.
    ThreadPool,
}

/// Options used to create a [`Heap`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options {
    kind: Kind,
    domain: Option<Domain>,
    creation_policy: CreationPolicy,
}

impl Options {
    /// Creates options for `kind`.
    #[must_use]
    pub const fn new(kind: Kind) -> Self {
        Self {
            kind,
            domain: None,
            creation_policy: CreationPolicy::Fresh,
        }
    }

    /// Creates general-purpose heap options.
    #[must_use]
    pub const fn general(options: general::Options) -> Self {
        Self::new(Kind::General(options))
    }

    /// Creates bump heap options.
    #[must_use]
    pub const fn bump(options: bump::Options) -> Self {
        Self::new(Kind::Bump(options))
    }

    /// Returns the selected heap kind.
    #[must_use]
    pub const fn kind(self) -> Kind {
        self.kind
    }

    /// Assigns the heap to `domain`.
    #[must_use]
    pub fn with_domain(mut self, domain: impl Into<Domain>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Requests reusable backing state from the backend's thread-local pool.
    ///
    /// Pooling is intended for bump heaps. Backends may reject this policy for
    /// other heap kinds.
    #[must_use]
    pub const fn with_thread_pool(mut self) -> Self {
        self.creation_policy = CreationPolicy::ThreadPool;
        self
    }

    /// Returns the selected domain, if any.
    #[must_use]
    pub const fn domain(self) -> Option<Domain> {
        self.domain
    }

    /// Returns the backing-state creation policy.
    #[must_use]
    pub const fn creation_policy(self) -> CreationPolicy {
        self.creation_policy
    }
}

impl Default for Options {
    fn default() -> Self {
        Self::general(general::Options::new())
    }
}

/// Cheap identity and configuration information for a heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Info {
    active: bool,
    domain: Domain,
    kind: InfoKind,
}

/// Heap-kind-specific identity and configuration information.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InfoKind {
    /// General-purpose heap information.
    General(general::Info),
    /// Bump heap information.
    Bump(bump::Info),
}

impl Info {
    /// Returns whether the heap is active.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Returns heap-kind-specific information.
    #[must_use]
    pub const fn kind(&self) -> &InfoKind {
        &self.kind
    }

    /// Returns the domain that supplies this heap's regions.
    #[must_use]
    pub const fn domain(&self) -> Domain {
        self.domain
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn new(active: bool, domain: Domain, kind: InfoKind) -> Self {
        Self { active, domain, kind }
    }
}

/// A consistent snapshot of a heap's current resource usage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Usage {
    live_allocations: usize,
    live_requested_bytes: usize,
    live_usable_bytes: usize,
    reserved_bytes: usize,
    committed_bytes: usize,
    kind: UsageKind,
}

/// Heap-kind-specific usage information.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UsageKind {
    /// General-purpose heap usage.
    General(general::Usage),
    /// Bump heap usage.
    Bump(bump::Usage),
}

impl Usage {
    /// Returns heap-kind-specific usage.
    #[must_use]
    pub const fn kind(&self) -> &UsageKind {
        &self.kind
    }

    /// Returns general-purpose usage when this is a general heap.
    #[must_use]
    pub const fn general(&self) -> Option<&general::Usage> {
        match &self.kind {
            UsageKind::General(usage) => Some(usage),
            UsageKind::Bump(_) => None,
        }
    }

    /// Returns bump usage when this is a bump heap.
    #[must_use]
    pub const fn bump(&self) -> Option<&bump::Usage> {
        match &self.kind {
            UsageKind::General(_) => None,
            UsageKind::Bump(usage) => Some(usage),
        }
    }

    /// Returns whether the heap has no live allocations.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.live_allocations == 0
    }

    /// Returns the number of live allocations.
    #[must_use]
    pub const fn live_allocations(&self) -> usize {
        self.live_allocations
    }

    /// Returns live requested bytes.
    #[must_use]
    pub const fn live_requested_bytes(&self) -> usize {
        self.live_requested_bytes
    }

    /// Returns live usable bytes.
    #[must_use]
    pub const fn live_usable_bytes(&self) -> usize {
        self.live_usable_bytes
    }

    /// Returns reserved bytes.
    #[must_use]
    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }

    /// Returns committed bytes.
    #[must_use]
    pub const fn committed_bytes(&self) -> usize {
        self.committed_bytes
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        live_allocations: usize,
        live_requested_bytes: usize,
        live_usable_bytes: usize,
        reserved_bytes: usize,
        committed_bytes: usize,
        kind: UsageKind,
    ) -> Self {
        Self {
            live_allocations,
            live_requested_bytes,
            live_usable_bytes,
            reserved_bytes,
            committed_bytes,
            kind,
        }
    }
}

/// An error produced while creating a heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CreationError {
    /// No process allocator registered a heap backend.
    BackendUnavailable,
    /// The installed backend could not create the requested heap.
    CreationFailed,
}

impl fmt::Display for CreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendUnavailable => formatter.write_str("no allocation heap backend is installed"),
            Self::CreationFailed => formatter.write_str("the allocation backend could not create the heap"),
        }
    }
}

impl std::error::Error for CreationError {}

/// Shared ownership and activation control for an allocator-native heap.
///
/// A heap is `Send` but not `Sync`; exclusive heap kinds can be active in only
/// one thread or scope at a time.
pub struct Heap {
    pub(crate) id: HeapId,
    not_sync: PhantomData<Cell<()>>,
}

/// An opaque identity that keeps its heap identity unique while retained.
///
/// Clones compare equal and keep the underlying identity alive. A newly created
/// heap cannot receive the same identity while any `HeapId` for an older heap
/// remains alive.
pub struct HeapId {
    control: Arc<HeapControl>,
}

struct HeapControl {
    active: AtomicBool,
    claim_policy: ClaimPolicy,
    hint: RawHint,
    backend: &'static Backend,
}

struct ReleaseClaim<'a>(&'a HeapId);

impl Heap {
    /// Creates a fresh general-purpose heap with standard options.
    ///
    /// # Panics
    ///
    /// Panics when no backend is installed or heap creation fails.
    #[must_use]
    pub fn new() -> Self {
        Self::with_options(Options::default())
    }

    /// Attempts to create a fresh general-purpose heap with standard options.
    ///
    /// # Errors
    ///
    /// Returns an error when no backend is installed or heap creation fails.
    pub fn try_new() -> Result<Self, CreationError> {
        Self::try_with_options(Options::default())
    }

    /// Creates a fresh bump heap.
    ///
    /// # Panics
    ///
    /// Panics when no backend is installed or heap creation fails.
    #[must_use]
    pub fn bump(options: bump::Options) -> Self {
        Self::with_options(Options::bump(options))
    }

    /// Attempts to create a fresh bump heap.
    ///
    /// # Errors
    ///
    /// Returns an error when no backend is installed or heap creation fails.
    pub fn try_bump(options: bump::Options) -> Result<Self, CreationError> {
        Self::try_with_options(Options::bump(options))
    }

    /// Creates a general-purpose or bump heap.
    ///
    /// # Panics
    ///
    /// Panics when [`Heap::try_with_options`] returns an error.
    #[must_use]
    pub fn with_options(options: Options) -> Self {
        Self::try_with_options(options).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Attempts to create a general-purpose or bump heap.
    ///
    /// # Errors
    ///
    /// Returns an error when no backend is installed or it cannot create the
    /// requested heap.
    ///
    /// # Panics
    ///
    /// Panics if the installed backend violates its contract by returning the
    /// process-global target as a heap.
    pub fn try_with_options(options: Options) -> Result<Self, CreationError> {
        let backend = installed_backend().ok_or(CreationError::BackendUnavailable)?;
        let raw = (backend.create)(options)?;
        assert!(!raw.hint.is_global(), "a heap target must not be global");
        Ok(Self::from_control(raw, backend))
    }

    /// Obtains pooled bump backing state from the current thread or creates it.
    ///
    /// # Panics
    ///
    /// Panics when the backend cannot provide the requested heap.
    #[must_use]
    pub fn from_thread_pool(options: bump::Options) -> Self {
        Self::with_options(Options::bump(options).with_thread_pool())
    }

    /// Obtains pooled bump backing state belonging to `domain` or creates it.
    ///
    /// # Panics
    ///
    /// Panics when the backend cannot provide the requested heap.
    pub fn from_thread_pool_in(domain: impl Into<Domain>, options: bump::Options) -> Self {
        Self::with_options(Options::bump(options).with_domain(domain).with_thread_pool())
    }

    /// Attempts to obtain pooled bump backing state from the current thread.
    ///
    /// # Errors
    ///
    /// Returns an error when no backend is installed or it cannot provide the
    /// requested heap.
    pub fn try_from_thread_pool(options: bump::Options) -> Result<Self, CreationError> {
        Self::try_with_options(Options::bump(options).with_thread_pool())
    }

    /// Attempts to obtain pooled bump backing state belonging to `domain`.
    ///
    /// # Errors
    ///
    /// Returns an error when no backend is installed or it cannot provide the
    /// requested heap.
    pub fn try_from_thread_pool_in(domain: impl Into<Domain>, options: bump::Options) -> Result<Self, CreationError> {
        Self::try_with_options(Options::bump(options).with_domain(domain).with_thread_pool())
    }

    /// Wraps a stable allocator-native heap target.
    ///
    /// # Safety
    ///
    /// `hint` must belong to `backend` and remain valid until the backend's
    /// destruction callback runs. `claim_policy` must accurately describe
    /// whether simultaneous scoped activation is safe. The backend must satisfy
    /// [`Backend::new`]'s callback invariants.
    ///
    /// # Panics
    ///
    /// Panics if another backend is registered or `hint` is the process-global
    /// target.
    #[doc(hidden)]
    #[must_use]
    pub unsafe fn from_raw(hint: RawHint, backend: &'static Backend, claim_policy: ClaimPolicy) -> Self {
        unsafe { backend::register(backend) };
        assert!(!hint.is_global(), "a heap target must not be global");
        Self::from_control(unsafe { RawHeap::new(hint, claim_policy) }, backend)
    }

    fn from_control(raw: RawHeap, backend: &'static Backend) -> Self {
        Self {
            id: HeapId {
                control: Arc::new(HeapControl {
                    active: AtomicBool::new(false),
                    claim_policy: raw.claim_policy,
                    hint: raw.hint,
                    backend,
                }),
            },
            not_sync: PhantomData,
        }
    }

    /// Returns an opaque stable identity for this heap.
    #[must_use]
    pub fn id(&self) -> HeapId {
        self.id.clone()
    }

    /// Returns a process-local numeric representation of this heap's identity.
    ///
    /// The value remains collision-free while this heap or any [`HeapId`]
    /// returned by [`Heap::id`] remains alive. Prefer `id` when retaining or
    /// comparing identities.
    #[must_use]
    pub fn identity(&self) -> usize {
        self.id.identity()
    }

    /// Returns whether this heap currently holds an exclusive activation claim.
    #[must_use]
    pub fn is_claimed(&self) -> bool {
        self.id.control.active.load(Ordering::Acquire)
    }

    /// Returns cheap identity and configuration information.
    #[must_use]
    pub fn info(&self) -> Info {
        self.with_exclusive_access(|control, active_here| unsafe { (control.backend.info)(control.hint, active_here) })
    }

    /// Tries to return cheap identity and configuration information without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when an exclusive heap is active on another thread.
    pub fn try_info(&self) -> Result<Info, crate::Error> {
        self.try_with_exclusive_access(|control, active_here| unsafe { (control.backend.info)(control.hint, active_here) })
            .ok_or_else(crate::Error::inspection_contended)
    }

    /// Returns a consistent usage snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when a thread heap is queried outside its owner thread.
    pub fn usage(&self) -> Result<Usage, crate::Error> {
        self.with_exclusive_access(|control, _| unsafe { (control.backend.usage)(control.hint) })
            .map_err(|()| crate::Error::usage_unavailable())
    }

    /// Tries to return a consistent usage snapshot without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when an exclusive heap is active on another thread or
    /// when a thread heap is queried outside its owner thread.
    pub fn try_usage(&self) -> Result<Usage, crate::Error> {
        self.try_with_exclusive_access(|control, _| unsafe { (control.backend.usage)(control.hint) })
            .ok_or_else(crate::Error::inspection_contended)?
            .map_err(|()| crate::Error::usage_unavailable())
    }

    /// Runs `inspect` while holding this heap's exclusive claim when required.
    ///
    /// A heap already active on this thread is inspected directly.
    fn with_exclusive_access<R>(&self, inspect: impl FnOnce(&HeapControl, bool) -> R) -> R {
        let mut inspect = Some(inspect);
        if let Some(result) = self.try_with_exclusive_access(|control, active_here| {
            inspect.take().expect("inspection callback is invoked at most once")(control, active_here)
        }) {
            return result;
        }

        let inspect = inspect.expect("a contended nonblocking inspection does not invoke its callback");
        let mut spins = 0;
        while !self.id.claim() {
            if spins < 64 {
                std::hint::spin_loop();
                spins += 1;
            } else {
                std::thread::yield_now();
            }
        }
        let _release = ReleaseClaim(&self.id);
        inspect(&self.id.control, false)
    }

    fn try_with_exclusive_access<R>(&self, inspect: impl FnOnce(&HeapControl, bool) -> R) -> Option<R> {
        let active_here = crate::is_claimed(self.id.identity());
        if active_here || self.id.claim_policy() == ClaimPolicy::Shared {
            return Some(inspect(&self.id.control, active_here));
        }
        if !self.id.claim() {
            return None;
        }
        let _release = ReleaseClaim(&self.id);
        Some(inspect(&self.id.control, false))
    }
}

/// Returns a handle that lets other threads allocate for the current thread.
///
/// Returns `None` when no supporting allocator backend is installed or the
/// backend cannot create the required process-retained queue metadata.
///
/// # Panics
///
/// Panics if the installed backend violates its contract by returning the
/// process-global target as a thread heap.
#[must_use]
pub fn thread_heap() -> Option<Heap> {
    let backend = installed_backend()?;
    let raw = (backend.thread_heap)()?;
    assert!(!raw.hint.is_global(), "a thread heap target must not be global");
    Some(Heap::from_control(raw, backend))
}

fn installed_backend() -> Option<&'static Backend> {
    backend::installed()
}

impl Default for Heap {
    /// Creates a fresh general-purpose heap with standard options.
    ///
    /// Equivalent to [`Heap::new`].
    ///
    /// # Panics
    ///
    /// Panics when no backend is installed or heap creation fails.
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<Self> for Heap {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl fmt::Debug for Heap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Heap").field("identity", &self.id.identity()).finish()
    }
}

impl PartialEq for HeapId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.control, &other.control)
    }
}

impl Eq for HeapId {}

impl Hash for HeapId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identity().hash(state);
    }
}

impl fmt::Debug for HeapId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("HeapId").field(&self.identity()).finish()
    }
}

impl Drop for ReleaseClaim<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl HeapId {
    pub(crate) fn raw_hint(&self) -> RawHint {
        self.control.hint
    }

    pub(crate) fn backend(&self) -> &'static Backend {
        self.control.backend
    }

    pub(crate) fn claim_policy(&self) -> ClaimPolicy {
        self.control.claim_policy
    }

    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.control).addr()
    }

    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn claim(&self) -> bool {
        self.control
            .active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    pub(crate) fn release(&self) {
        let was_active = self.control.active.swap(false, Ordering::Release);
        debug_assert!(was_active);
    }
}

impl Clone for HeapId {
    fn clone(&self) -> Self {
        Self {
            control: Arc::clone(&self.control),
        }
    }
}

impl Drop for HeapControl {
    fn drop(&mut self) {
        debug_assert!(!self.active.load(Ordering::Relaxed));
        unsafe { (self.backend.destroy)(self.hint) };
    }
}
