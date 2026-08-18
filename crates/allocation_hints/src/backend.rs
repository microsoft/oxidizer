// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocator-backend integration plumbing.
//!
//! Application code should not need this module. Allocator implementations use
//! it to register the process-global callbacks that create and control native
//! heap and domain targets.

use std::ptr;
use std::sync::OnceLock;

use crate::heap::{CreationError, Info, Options, Usage};

static BACKEND: OnceLock<&'static Backend> = OnceLock::new();

/// The allocator-native target installed for the current hint scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawHint {
    target: *mut (),
    kind: usize,
}

// Backend targets are process-owned opaque identities. Backend registration
// guarantees that their callbacks support cross-thread heap ownership.
// SAFETY: RawHint is an opaque address and tag. Sending or sharing it does not
// dereference the address; the registered backend owns all access invariants.
unsafe impl Send for RawHint {}
// SAFETY: Sharing RawHint does not access its target; only the backend interprets it.
unsafe impl Sync for RawHint {}

impl RawHint {
    /// The default process-global allocation target.
    pub const GLOBAL: Self = {
        // SAFETY: A null target paired with kind zero is the defined global hint.
        unsafe { Self::new(ptr::null_mut(), 0) }
    };

    /// Creates a raw allocator-native target.
    ///
    /// # Safety
    ///
    /// The target and kind must form a valid pair for the backend receiving it.
    pub const unsafe fn new(target: *mut (), kind: usize) -> Self {
        Self { target, kind }
    }

    /// Returns the opaque allocator-native target.
    #[must_use]
    pub const fn target(self) -> *mut () {
        self.target
    }

    /// Returns the allocator-defined target kind.
    #[must_use]
    pub const fn kind(self) -> usize {
        self.kind
    }

    /// Returns whether this selects the default process-global target.
    #[must_use]
    pub const fn is_global(self) -> bool {
        self.target.is_null()
    }
}

/// An opaque backend-native domain target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawDomain {
    target: *mut (),
}

// SAFETY: RawDomain is an opaque backend identity. Only its originating
// backend interprets the address under that backend's lifetime contract.
unsafe impl Send for RawDomain {}
// SAFETY: Sharing RawDomain does not dereference the opaque backend target.
unsafe impl Sync for RawDomain {}

impl RawDomain {
    /// Creates a raw domain target.
    ///
    /// # Safety
    ///
    /// `target` must be a valid process-retained domain target for the backend
    /// receiving it.
    pub const unsafe fn new(target: *mut ()) -> Self {
        Self { target }
    }

    /// Returns the opaque target.
    #[must_use]
    pub const fn target(self) -> *mut () {
        self.target
    }
}

/// The concurrency policy for scoped heap activation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimPolicy {
    /// The target may be active concurrently.
    Shared,
    /// The target may be active on only one thread or outermost scope.
    Exclusive,
}

/// A heap target returned by a backend factory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawHeap {
    pub(crate) hint: RawHint,
    pub(crate) claim_policy: ClaimPolicy,
}

impl RawHeap {
    /// Creates a backend heap target.
    ///
    /// # Safety
    ///
    /// `hint` must be a valid non-global target and `claim_policy` must describe
    /// its actual concurrency requirements.
    #[must_use]
    pub const unsafe fn new(hint: RawHint, claim_policy: ClaimPolicy) -> Self {
        Self { hint, claim_policy }
    }
}

/// Type-erased allocator operations used by heap construction and scoped hints.
#[derive(Debug)]
pub struct Backend {
    pub(crate) create_domain: fn() -> Option<RawDomain>,
    pub(crate) default_domain: fn() -> RawDomain,
    pub(crate) create: fn(Options) -> Result<RawHeap, CreationError>,
    pub(crate) thread_heap: fn() -> Option<RawHeap>,
    pub(crate) thread_context: fn() -> *mut (),
    pub(crate) activate: unsafe fn(*mut (), RawHint),
    pub(crate) info: unsafe fn(RawHint, bool) -> Info,
    pub(crate) usage: unsafe fn(RawHint) -> Result<Usage, ()>,
    pub(crate) destroy: unsafe fn(RawHint),
}

impl Backend {
    /// Creates a type-erased allocator backend.
    ///
    /// # Safety
    ///
    /// All callbacks must agree on native target representations and lifetimes.
    /// A successful `create_domain` callback and every `default_domain` callback
    /// must return non-null, process-retained targets. Each live target returned
    /// by `create_domain` must be distinct from every other live domain target,
    /// including the target returned by `default_domain`, and `default_domain`
    /// must always return the same target. A successful `thread_heap` callback
    /// must return a process-retained target associated with the calling thread.
    /// Thread contexts must remain valid for their thread's lifetime. Callbacks
    /// must not unwind or reenter this crate.
    /// Activation callbacks run only on the thread owning their thread context;
    /// creation, inspection, and destruction may run concurrently on arbitrary
    /// threads. Every target returned by `create` must therefore be movable
    /// between threads and remain valid until its one destruction callback.
    /// Inspection must tolerate the concurrency permitted by its
    /// [`ClaimPolicy`].
    #[expect(clippy::too_many_arguments, reason = "The backend ABI is a flat callback table")]
    pub const unsafe fn new(
        create_domain: fn() -> Option<RawDomain>,
        default_domain: fn() -> RawDomain,
        create: fn(Options) -> Result<RawHeap, CreationError>,
        thread_heap: fn() -> Option<RawHeap>,
        thread_context: fn() -> *mut (),
        activate: unsafe fn(*mut (), RawHint),
        info: unsafe fn(RawHint, bool) -> Info,
        usage: unsafe fn(RawHint) -> Result<Usage, ()>,
        destroy: unsafe fn(RawHint),
    ) -> Self {
        Self {
            create_domain,
            default_domain,
            create,
            thread_heap,
            thread_context,
            activate,
            info,
            usage,
            destroy,
        }
    }
}

/// Registers the process-global allocation backend.
///
/// Re-registering the same static backend is accepted. Registering a different
/// backend panics.
///
/// # Safety
///
/// The backend must satisfy the invariants of [`Backend::new`] for every target
/// it creates, and the process must not use a different allocation backend.
///
/// # Panics
///
/// Panics if a different backend was already registered.
#[doc(hidden)]
pub unsafe fn register(backend: &'static Backend) {
    if let Some(current) = BACKEND.get() {
        assert!(
            ptr::eq(*current, backend),
            "a different allocation heap backend is already installed"
        );
        return;
    }
    complete_registration(&BACKEND, backend);
}

pub(crate) fn complete_registration(slot: &OnceLock<&'static Backend>, backend: &'static Backend) {
    if slot.set(backend).is_err() {
        let current = slot.get().expect("backend registration raced");
        assert!(
            ptr::eq(*current, backend),
            "a different allocation heap backend is already installed"
        );
    }
}

pub(crate) fn installed() -> Option<&'static Backend> {
    BACKEND.get().copied()
}
