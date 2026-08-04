//! Allocator-independent heap ownership, configuration, and inspection.

pub mod bump;
pub mod general;

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backend::{self, Backend, ClaimPolicy, RawHeap, RawHint};
use crate::domain::Domain;

/// The allocation strategy selected for a [`Heap`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// A general-purpose heap.
    General(general::Options),
    /// A bump heap.
    Bump(bump::Options),
}

/// The backing-state creation policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    pub const fn new(kind: Kind) -> Self {
        Self {
            kind,
            domain: None,
            creation_policy: CreationPolicy::Fresh,
        }
    }

    /// Creates general-purpose heap options.
    pub const fn general(options: general::Options) -> Self {
        Self::new(Kind::General(options))
    }

    /// Creates bump heap options.
    pub const fn bump(options: bump::Options) -> Self {
        Self::new(Kind::Bump(options))
    }

    /// Returns the selected heap kind.
    pub const fn kind(self) -> Kind {
        self.kind
    }

    /// Assigns the heap to `domain`.
    pub fn with_domain(mut self, domain: impl Into<Domain>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Requests reusable backing state from the backend's thread-local pool.
    pub const fn with_thread_pool(mut self) -> Self {
        self.creation_policy = CreationPolicy::ThreadPool;
        self
    }

    /// Returns the selected domain, if any.
    pub const fn domain(self) -> Option<Domain> {
        self.domain
    }

    /// Returns the backing-state creation policy.
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
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Returns heap-kind-specific information.
    pub const fn kind(&self) -> &InfoKind {
        &self.kind
    }

    /// Returns the domain that supplies this heap's regions.
    pub const fn domain(&self) -> Domain {
        self.domain
    }

    #[doc(hidden)]
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
    pub const fn kind(&self) -> &UsageKind {
        &self.kind
    }

    /// Returns general-purpose usage when this is a general heap.
    pub const fn general(&self) -> Option<&general::Usage> {
        match &self.kind {
            UsageKind::General(usage) => Some(usage),
            UsageKind::Bump(_) => None,
        }
    }

    /// Returns bump usage when this is a bump heap.
    pub const fn bump(&self) -> Option<&bump::Usage> {
        match &self.kind {
            UsageKind::General(_) => None,
            UsageKind::Bump(usage) => Some(usage),
        }
    }

    /// Returns whether the heap has no live allocations.
    pub const fn is_empty(&self) -> bool {
        self.live_allocations == 0
    }

    /// Returns the number of live allocations.
    pub const fn live_allocations(&self) -> usize {
        self.live_allocations
    }

    /// Returns live requested bytes.
    pub const fn live_requested_bytes(&self) -> usize {
        self.live_requested_bytes
    }

    /// Returns live usable bytes.
    pub const fn live_usable_bytes(&self) -> usize {
        self.live_usable_bytes
    }

    /// Returns reserved bytes.
    pub const fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }

    /// Returns committed bytes.
    pub const fn committed_bytes(&self) -> usize {
        self.committed_bytes
    }

    #[doc(hidden)]
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

pub(crate) struct HeapId {
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
    pub fn new() -> Self {
        Self::with_options(Options::default())
    }

    /// Attempts to create a fresh general-purpose heap with standard options.
    pub fn try_new() -> Result<Self, CreationError> {
        Self::try_with_options(Options::default())
    }

    /// Creates a fresh bump heap.
    pub fn bump(options: bump::Options) -> Self {
        Self::with_options(Options::bump(options))
    }

    /// Attempts to create a fresh bump heap.
    pub fn try_bump(options: bump::Options) -> Result<Self, CreationError> {
        Self::try_with_options(Options::bump(options))
    }

    /// Creates a general-purpose or bump heap.
    pub fn with_options(options: Options) -> Self {
        Self::try_with_options(options).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Attempts to create a general-purpose or bump heap.
    pub fn try_with_options(options: Options) -> Result<Self, CreationError> {
        let backend = installed_backend().ok_or(CreationError::BackendUnavailable)?;
        let raw = (backend.create)(options)?;
        assert!(!raw.hint.is_global(), "a heap target must not be global");
        Ok(Self::from_control(raw, backend))
    }

    /// Obtains pooled bump backing state from the current thread or creates it.
    pub fn from_thread_pool(options: bump::Options) -> Self {
        Self::with_options(Options::bump(options).with_thread_pool())
    }

    /// Obtains pooled bump backing state belonging to `domain` or creates it.
    pub fn from_thread_pool_in(domain: impl Into<Domain>, options: bump::Options) -> Self {
        Self::with_options(Options::bump(options).with_domain(domain).with_thread_pool())
    }

    /// Wraps a stable allocator-native heap target.
    ///
    /// # Safety
    ///
    /// `hint` must belong to `backend` and remain valid until the backend's
    /// destruction callback runs. `claim_policy` must accurately describe
    /// whether simultaneous scoped activation is safe. The backend must satisfy
    /// [`Backend::new`]'s callback invariants.
    #[doc(hidden)]
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

    /// Returns the stable identity of this heap.
    pub fn identity(&self) -> usize {
        self.id.identity()
    }

    /// Returns whether this heap currently holds an exclusive activation claim.
    pub fn is_claimed(&self) -> bool {
        self.id.control.active.load(Ordering::Acquire)
    }

    /// Returns cheap identity and configuration information.
    pub fn info(&self) -> Info {
        self.with_exclusive_access(|control, active_here| unsafe { (control.backend.info)(control.hint, active_here) })
    }

    /// Returns a consistent usage snapshot.
    pub fn usage(&self) -> Result<Usage, crate::Error> {
        self.with_exclusive_access(|control, _| unsafe { (control.backend.usage)(control.hint) })
            .map_err(|()| crate::Error::usage_unavailable())
    }

    /// Runs `inspect` while holding this heap's exclusive claim when required.
    ///
    /// A heap already active on this thread is inspected directly.
    fn with_exclusive_access<R>(&self, inspect: impl FnOnce(&HeapControl, bool) -> R) -> R {
        let active_here = crate::is_claimed(self.id.identity());
        if active_here || self.id.claim_policy() == ClaimPolicy::Shared {
            return inspect(&self.id.control, active_here);
        }
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
}

/// Returns a handle that lets other threads allocate for the current thread.
///
/// Returns `None` when no supporting allocator backend is installed or the
/// backend cannot create the required process-retained queue metadata.
pub fn thread_heap() -> Option<Heap> {
    let backend = installed_backend()?;
    let raw = (backend.thread_heap)()?;
    assert!(!raw.hint.is_global(), "a thread heap target must not be global");
    Some(Heap::from_control(raw, backend))
}

fn installed_backend() -> Option<&'static Backend> {
    // Allocating this box can lazily initialize the process allocator and
    // register its backend before the registry is inspected.
    let registration_probe = Box::<u8>::new_uninit();
    let backend = backend::installed();
    drop(registration_probe);
    backend
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<Heap> for Heap {
    fn as_ref(&self) -> &Heap {
        self
    }
}

impl fmt::Debug for Heap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Heap").field("identity", &self.id.identity()).finish()
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
