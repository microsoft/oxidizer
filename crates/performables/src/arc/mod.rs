// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry-enabled reference-counted ownership.

mod factory;

use std::any::Any;
use std::borrow::{Borrow, Cow};
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc as StdArc, RwLock, Weak as StdWeak};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thread_aware::{Owner, Thread, ThreadAware};

use self::factory::Factory;
use crate::telemetry::{self, EventKind};

/// Shares one value across the process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PerProcess;

/// Materializes and shares one value per runtime thread.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PerCore;

/// Materializes and shares one value per NUMA node.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PerNuma;

mod private {
    use std::fmt::Debug;
    use std::hash::Hash;
    use std::thread::ThreadId;

    use thread_aware::{NumaNode, Thread};

    pub trait Sealed {}

    impl Sealed for super::PerCore {}
    impl Sealed for super::PerNuma {}
    impl Sealed for super::PerProcess {}

    pub trait AffinityStrategy: Sealed {
        type Key: Clone + Debug + Eq + Hash + Send + Sync + 'static;

        fn key(thread: &Thread) -> Self::Key;
    }

    impl AffinityStrategy for super::PerCore {
        type Key = ThreadId;

        fn key(thread: &Thread) -> Self::Key {
            thread.id()
        }
    }

    impl AffinityStrategy for super::PerNuma {
        type Key = NumaNode;

        fn key(thread: &Thread) -> Self::Key {
            thread.numa_node().clone()
        }
    }
}

/// Internal representation contract implemented by built-in Arc strategies.
#[doc(hidden)]
pub trait Strategy<T: ?Sized>: private::Sealed {
    /// Strategy-owned state stored inline by [`Arc`].
    type State;

    /// Returns the current allocation.
    fn current(state: &Self::State) -> &StdArc<T>;

    /// Clones the strategy state.
    fn clone_state(state: &Self::State) -> Self::State;

    /// Returns the logical strong-reference count.
    fn strong_count(state: &Self::State) -> usize;

    /// Consumes the state and returns its current allocation.
    fn into_current(state: Self::State) -> StdArc<T>;

    /// Creates strategy-owned state with a sized constructor.
    fn from_constructor(constructor: fn() -> T) -> Self::State
    where
        T: Sized + 'static;

    /// Creates strategy-owned state with a boxed constructor.
    fn from_boxed_constructor(constructor: fn() -> Box<T>) -> Self::State
    where
        T: 'static;

    /// Creates strategy-owned state from explicit constructor data.
    fn from_constructor_data<D>(data: D, constructor: fn(D) -> T) -> Self::State
    where
        T: Sized + 'static,
        D: ThreadAware + Clone + Sync + 'static;

    /// Creates strategy-owned state by cloning without relocation.
    fn from_unaware(value: T) -> Self::State
    where
        T: Sized + Clone + Send + 'static;

    /// Creates strategy-owned state from a concrete clone template.
    fn from_clone_function<V>(value: V, clone_function: fn(&V) -> Box<T>) -> Self::State
    where
        T: ThreadAware + 'static,
        V: Send + Sync + 'static;
}

/// Internal cloning contract for process-wide [`Arc::make_mut`].
#[doc(hidden)]
pub trait MakeMutTarget {
    /// Clones the allocation when necessary and returns unique mutable access.
    fn make_mut(arc: &mut StdArc<Self>) -> &mut Self;
}

impl<T: Clone> MakeMutTarget for T {
    fn make_mut(arc: &mut StdArc<Self>) -> &mut Self {
        StdArc::make_mut(arc)
    }
}

impl<T: Clone> MakeMutTarget for [T] {
    fn make_mut(arc: &mut StdArc<Self>) -> &mut Self {
        StdArc::make_mut(arc)
    }
}

impl MakeMutTarget for str {
    fn make_mut(arc: &mut StdArc<Self>) -> &mut Self {
        StdArc::make_mut(arc)
    }
}

/// A strategy-based reference-counting pointer that records ownership operations.
///
/// `Arc<T, PerProcess>` has the same representation size as
/// `std::sync::Arc<T>`. Per-core and per-NUMA strategies keep their factory and
/// affinity storage in strategy-owned state shared by all clones. Only those
/// affinity-backed strategies implement [`ThreadAware`]; `PerProcess` does not.
///
/// ```compile_fail
/// use performables::arc::{Arc, PerProcess};
/// use thread_aware::ThreadAware;
///
/// fn require_thread_aware<T: ThreadAware>() {}
///
/// require_thread_aware::<Arc<u64, PerProcess>>();
/// ```
pub struct Arc<T: ?Sized, S = PerProcess>
where
    S: Strategy<T>,
{
    state: S::State,
    marker: PhantomData<StdArc<T>>,
}

/// A non-owning reference to a process-wide [`Arc`] allocation.
pub struct Weak<T: ?Sized> {
    inner: StdWeak<T>,
}

#[doc(hidden)]
pub struct AffinityState<T: ?Sized, S: private::AffinityStrategy> {
    shared: StdArc<AffinityShared<T, S>>,
    current: StdArc<T>,
    current_owner: Option<Owner>,
}

struct AffinityShared<T: ?Sized, S: private::AffinityStrategy> {
    inner: RwLock<AffinityInner<T, S>>,
}

struct AffinityInner<T: ?Sized, S: private::AffinityStrategy> {
    storage: AffinityStorage<StdArc<T>, S>,
    factory: Option<Factory<T>>,
}

#[derive(Debug)]
struct AffinityStorage<T, S: private::AffinityStrategy> {
    owner: Option<Owner>,
    values: HashMap<S::Key, T>,
}

/// Failure to construct an affinity-backed [`Arc`] from prebuilt values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// A supplied thread belongs to a different runtime owner.
    ForeignOwner,
    /// More than one value was supplied for the same strategy partition.
    Duplicate,
    /// No value was supplied for the current strategy partition.
    MissingCurrent,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignOwner => f.write_str("a supplied thread belongs to a different runtime owner"),
            Self::Duplicate => f.write_str("multiple values were supplied for the same thread partition"),
            Self::MissingCurrent => f.write_str("no value was supplied for the current thread partition"),
        }
    }
}

impl std::error::Error for Error {}

impl<T: ?Sized, S: private::AffinityStrategy> fmt::Debug for AffinityState<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AffinityState").finish_non_exhaustive()
    }
}

impl<T, S: private::AffinityStrategy> AffinityStorage<T, S> {
    fn new() -> Self {
        Self {
            owner: None,
            values: HashMap::new(),
        }
    }

    fn bind_owner(&mut self, owner: &Owner) -> bool {
        if let Some(bound) = &self.owner {
            bound == owner
        } else {
            self.owner = Some(owner.clone());
            true
        }
    }

    fn get_clone(&self, key: &S::Key) -> Option<T>
    where
        T: Clone,
    {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: S::Key, value: T) -> Result<(), T> {
        match self.values.entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => Err(value),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value);
                Ok(())
            }
        }
    }

    fn count_where(&self, predicate: impl Fn(&T) -> bool) -> usize {
        self.values.values().filter(|value| predicate(value)).count()
    }
}

impl<T: ?Sized> Strategy<T> for PerProcess {
    type State = StdArc<T>;

    fn current(state: &Self::State) -> &StdArc<T> {
        state
    }

    fn clone_state(state: &Self::State) -> Self::State {
        StdArc::clone(state)
    }

    fn strong_count(state: &Self::State) -> usize {
        StdArc::strong_count(state)
    }

    fn into_current(state: Self::State) -> StdArc<T> {
        state
    }

    fn from_constructor(constructor: fn() -> T) -> Self::State
    where
        T: Sized + 'static,
    {
        StdArc::new(constructor())
    }

    fn from_boxed_constructor(constructor: fn() -> Box<T>) -> Self::State
    where
        T: 'static,
    {
        StdArc::from(constructor())
    }

    fn from_constructor_data<D>(data: D, constructor: fn(D) -> T) -> Self::State
    where
        T: Sized + 'static,
        D: ThreadAware + Clone + Sync + 'static,
    {
        StdArc::new(constructor(data))
    }

    fn from_unaware(value: T) -> Self::State
    where
        T: Sized + Clone + Send + 'static,
    {
        StdArc::new(value)
    }

    fn from_clone_function<V>(value: V, clone_function: fn(&V) -> Box<T>) -> Self::State
    where
        T: ThreadAware + 'static,
        V: Send + Sync + 'static,
    {
        StdArc::from(clone_function(&value))
    }
}

impl<T: ?Sized, S: private::AffinityStrategy> Strategy<T> for S {
    type State = AffinityState<T, S>;

    fn current(state: &Self::State) -> &StdArc<T> {
        &state.current
    }

    fn clone_state(state: &Self::State) -> Self::State {
        AffinityState {
            shared: StdArc::clone(&state.shared),
            current: StdArc::clone(&state.current),
            current_owner: state.current_owner.clone(),
        }
    }

    fn strong_count(state: &Self::State) -> usize {
        let raw = StdArc::strong_count(&state.current);
        let inner = state.shared.inner.read().expect("Arc affinity state lock was poisoned");
        let internal = inner.storage.count_where(|stored| StdArc::ptr_eq(stored, &state.current));
        raw.saturating_sub(internal)
    }

    fn into_current(state: Self::State) -> StdArc<T> {
        state.current
    }

    fn from_constructor(constructor: fn() -> T) -> Self::State
    where
        T: Sized + 'static,
    {
        let (current, factory) = Factory::from_function(constructor);
        affinity_state(current, factory)
    }

    fn from_boxed_constructor(constructor: fn() -> Box<T>) -> Self::State
    where
        T: 'static,
    {
        let (current, factory) = Factory::from_boxed_function(constructor);
        affinity_state(current, factory)
    }

    fn from_constructor_data<D>(data: D, constructor: fn(D) -> T) -> Self::State
    where
        T: Sized + 'static,
        D: ThreadAware + Clone + Sync + 'static,
    {
        let (current, factory) = Factory::from_data(data, constructor);
        affinity_state(current, factory)
    }

    fn from_unaware(value: T) -> Self::State
    where
        T: Sized + Clone + Send + 'static,
    {
        affinity_state(StdArc::new(value), Factory::clone_current())
    }

    fn from_clone_function<V>(value: V, clone_function: fn(&V) -> Box<T>) -> Self::State
    where
        T: ThreadAware + 'static,
        V: Send + Sync + 'static,
    {
        let (current, factory) = Factory::from_clone_function(value, clone_function);
        affinity_state(current, factory)
    }
}

impl<T, S> ThreadAware for Arc<T, S>
where
    T: Send + Sync + ?Sized,
    S: private::AffinityStrategy,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        telemetry::record(EventKind::ArcRelocate, Self::as_ptr(self).cast::<()>());
        relocate_affinity::<T, S>(&mut self.state, source, destination);
    }
}

fn relocate_affinity<T: ?Sized, S: private::AffinityStrategy>(
    state: &mut AffinityState<T, S>,
    source: Option<&Thread>,
    destination: &Thread,
) {
    if let Some(source) = source
        && source.owner() != destination.owner()
    {
        state.current_owner.get_or_insert_with(|| source.owner().clone());
        return;
    }

    if state.current_owner.as_ref().is_some_and(|owner| owner != destination.owner()) {
        return;
    }

    let mut inner = state.shared.inner.write().expect("Arc affinity state lock was poisoned");
    if !inner.storage.bind_owner(destination.owner()) {
        return;
    }

    if let Some(factory) = inner.factory.as_mut() {
        factory.record_source(source);
    }

    let destination_key = S::key(destination);
    if let Some(value) = inner.storage.get_clone(&destination_key) {
        state.current = value;
        state.current_owner = Some(destination.owner().clone());
        return;
    }

    let source_key = source.map(S::key);
    if source_key.as_ref() == Some(&destination_key) {
        _ = inner.storage.insert(destination_key, StdArc::clone(&state.current));
        state.current_owner = Some(destination.owner().clone());
        return;
    }

    let next = inner.factory.as_ref().map_or_else(
        || StdArc::clone(&state.current),
        |factory| factory.materialize(&state.current, source, destination),
    );
    let previous = std::mem::replace(&mut state.current, next);
    state.current_owner = Some(destination.owner().clone());
    let result = inner.storage.insert(destination_key.clone(), StdArc::clone(&state.current));
    assert!(result.is_ok(), "destination was checked while holding the same write lock");
    if let Some(source_key) = source_key
        && source_key != destination_key
    {
        _ = inner.storage.insert(source_key, previous);
    }
}

impl<T> Arc<T, PerProcess> {
    /// Allocates `value` in a process-wide reference-counted allocation.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self::from_state(StdArc::new(value))
    }

    /// Allocates `value`, allowing it to retain a weak reference to itself.
    pub fn new_cyclic<F>(data_fn: F) -> Self
    where
        F: FnOnce(&Weak<T>) -> T,
    {
        Self::from_state(StdArc::new_cyclic(|weak| {
            data_fn(&Weak {
                inner: StdWeak::clone(weak),
            })
        }))
    }

    /// Constructs a pinned process-wide reference-counted allocation.
    #[must_use]
    pub fn pin(value: T) -> Pin<Self> {
        // SAFETY: the pointee is owned by an Arc allocation and cannot move.
        unsafe { Pin::new_unchecked(Self::new(value)) }
    }

    /// Returns the inner value when this is its only strong reference.
    ///
    /// # Errors
    ///
    /// Returns the original Arc when other strong references remain.
    pub fn try_unwrap(this: Self) -> Result<T, Self> {
        match StdArc::try_unwrap(Self::into_std_arc(this)) {
            Ok(value) => Ok(value),
            Err(state) => Err(Self::from_state_unrecorded(state)),
        }
    }

    /// Returns the inner value if this is its only strong reference.
    #[must_use]
    pub fn into_inner(this: Self) -> Option<T> {
        StdArc::into_inner(Self::into_std_arc(this))
    }

    /// Wraps an existing standard Arc.
    #[must_use]
    pub fn from_std(inner: StdArc<T>) -> Self {
        Self::from_state(inner)
    }

    /// Returns mutable access when this is the only strong reference.
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        StdArc::get_mut(&mut this.state)
    }

    /// Returns the inner value, cloning it when other references remain.
    #[must_use]
    pub fn unwrap_or_clone(this: Self) -> T
    where
        T: Clone,
    {
        StdArc::unwrap_or_clone(Self::into_std_arc(this))
    }
}

impl<T: ?Sized> Arc<T, PerProcess> {
    fn from_state_unrecorded(state: StdArc<T>) -> Self {
        Self {
            state,
            marker: PhantomData,
        }
    }

    /// Coerces this pointer to an unsized target through the standard library's Arc.
    #[must_use]
    pub fn coerce<U: ?Sized>(self, coerce: impl FnOnce(StdArc<T>) -> StdArc<U>) -> Arc<U, PerProcess> {
        Arc::from_state_unrecorded(coerce(Self::into_std_arc(self)))
    }

    /// Returns the number of weak references to this allocation.
    #[must_use]
    pub fn weak_count(this: &Self) -> usize {
        StdArc::weak_count(&this.state)
    }

    /// Creates a non-owning reference to this allocation.
    #[must_use]
    pub fn downgrade(this: &Self) -> Weak<T> {
        Weak {
            inner: StdArc::downgrade(&this.state),
        }
    }

    /// Converts this pointer into a raw pointer to its value.
    #[must_use]
    pub fn into_raw(this: Self) -> *const T {
        StdArc::into_raw(Self::into_std_arc(this))
    }

    /// Reconstructs an Arc previously returned by [`Arc::into_raw`].
    ///
    /// # Safety
    ///
    /// `pointer` must have been returned by `Arc::into_raw` for the same type.
    #[must_use]
    pub unsafe fn from_raw(pointer: *const T) -> Self {
        Self::from_state_unrecorded(
            // SAFETY: upheld by the caller.
            unsafe { StdArc::from_raw(pointer) },
        )
    }

    /// Increments the strong count represented by a raw pointer.
    ///
    /// # Safety
    ///
    /// The pointer must represent a live Arc allocation.
    pub unsafe fn increment_strong_count(pointer: *const T) {
        // SAFETY: upheld by the caller.
        unsafe { StdArc::increment_strong_count(pointer) };
    }

    /// Decrements the strong count represented by a raw pointer.
    ///
    /// # Safety
    ///
    /// The pointer must represent one owned strong reference.
    pub unsafe fn decrement_strong_count(pointer: *const T) {
        // SAFETY: upheld by the caller.
        unsafe { StdArc::decrement_strong_count(pointer) };
    }
}

impl<T: ?Sized + MakeMutTarget> Arc<T, PerProcess> {
    /// Makes the allocation uniquely owned and returns mutable access.
    pub fn make_mut(this: &mut Self) -> &mut T {
        T::make_mut(&mut this.state)
    }
}

impl<T> Arc<MaybeUninit<T>, PerProcess> {
    /// Allocates an uninitialized value.
    #[must_use]
    pub fn new_uninit() -> Self {
        Self::new(MaybeUninit::uninit())
    }

    /// Allocates a zero-initialized value.
    #[must_use]
    pub fn new_zeroed() -> Self {
        Self::new(MaybeUninit::zeroed())
    }

    /// Assumes the value has been initialized.
    ///
    /// # Safety
    ///
    /// The pointee must contain a valid initialized `T`.
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<T, PerProcess> {
        let state = Self::into_std_arc(self);
        Arc::from_state_unrecorded(
            // SAFETY: upheld by the caller.
            unsafe { StdArc::<MaybeUninit<T>>::assume_init(state) },
        )
    }
}

impl Arc<dyn Any + Send + Sync, PerProcess> {
    /// Attempts to downcast this Arc to a concrete type.
    ///
    /// # Errors
    ///
    /// Returns the original type-erased Arc when its value is not a `T`.
    pub fn downcast<T>(self) -> Result<Arc<T, PerProcess>, Self>
    where
        T: Any + Send + Sync,
    {
        match StdArc::downcast(Self::into_std_arc(self)) {
            Ok(state) => Ok(Arc::from_state_unrecorded(state)),
            Err(state) => Err(Self::from_state_unrecorded(state)),
        }
    }
}

impl<T, S> Arc<T, S>
where
    T: 'static,
    S: Strategy<T>,
{
    /// Creates a value using the strategy's constructor semantics.
    #[must_use]
    pub fn new_with(constructor: fn() -> T) -> Self {
        Self::from_state(S::from_constructor(constructor))
    }

    /// Creates a value from explicit constructor data using the strategy's semantics.
    ///
    /// The data must be safely relocatable so switching to an affinity-backed
    /// strategy does not change the API requirements.
    #[must_use]
    pub fn new_with_data<D>(data: D, constructor: fn(D) -> T) -> Self
    where
        D: ThreadAware + Clone + Sync + 'static,
    {
        Self::from_state(S::from_constructor_data(data, constructor))
    }
}

impl<T, S> Arc<T, S>
where
    T: 'static + ?Sized,
    S: Strategy<T>,
{
    /// Creates a boxed value using the strategy's constructor semantics.
    #[must_use]
    pub fn new_boxed(constructor: fn() -> Box<T>) -> Self {
        Self::from_state(S::from_boxed_constructor(constructor))
    }
}

impl<T, S> Arc<T, S>
where
    T: Clone + Send + 'static,
    S: Strategy<T>,
{
    /// Creates a value by cloning without relocation when the strategy needs another instance.
    #[must_use]
    pub fn from_unaware(value: T) -> Self {
        Self::from_state(S::from_unaware(value))
    }
}

impl<T, S> Arc<T, S>
where
    T: ThreadAware + 'static + ?Sized,
    S: Strategy<T>,
{
    /// Creates a value from a concrete clone template using the strategy's semantics.
    #[must_use]
    pub fn with_clone_fn<V>(value: V, clone_function: fn(&V) -> Box<T>) -> Self
    where
        V: Send + Sync + 'static,
    {
        Self::from_state(S::from_clone_function(value, clone_function))
    }
}

impl<T: ?Sized> Arc<T, PerCore> {
    /// Constructs a per-core pointer from prebuilt values for runtime threads.
    ///
    /// The values are sealed into the pointer's private thread-keyed storage.
    /// Threads owned by another runtime and duplicate thread partitions are
    /// rejected. Unlisted thread partitions reuse the currently carried value.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a supplied thread has a different owner, a
    /// partition is duplicated, or the current partition has no value.
    pub fn try_from_values<I>(current_thread: &Thread, values: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = (Thread, Arc<T, PerProcess>)>,
    {
        affinity_state_from_values::<T, PerCore, I>(current_thread, values).map(Self::from_state)
    }
}

impl<T: ?Sized> Arc<T, PerNuma> {
    /// Constructs a per-NUMA pointer from prebuilt values for NUMA nodes.
    ///
    /// The values are sealed into the pointer's private NUMA-node-keyed storage.
    /// Threads owned by another runtime and duplicate NUMA-node partitions are
    /// rejected. Unlisted NUMA-node partitions reuse the currently carried value.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a supplied thread has a different owner, a
    /// partition is duplicated, or the current partition has no value.
    pub fn try_from_values<I>(current_thread: &Thread, values: I) -> Result<Self, Error>
    where
        I: IntoIterator<Item = (Thread, Arc<T, PerProcess>)>,
    {
        affinity_state_from_values::<T, PerNuma, I>(current_thread, values).map(Self::from_state)
    }
}

fn affinity_state<T: ?Sized, S: private::AffinityStrategy>(current: StdArc<T>, factory: Factory<T>) -> AffinityState<T, S> {
    AffinityState {
        shared: StdArc::new(AffinityShared {
            inner: RwLock::new(AffinityInner {
                storage: AffinityStorage::new(),
                factory: Some(factory),
            }),
        }),
        current,
        current_owner: None,
    }
}

fn affinity_state_from_values<T: ?Sized, S, I>(current_thread: &Thread, values: I) -> Result<AffinityState<T, S>, Error>
where
    S: private::AffinityStrategy,
    I: IntoIterator<Item = (Thread, Arc<T, PerProcess>)>,
{
    let mut storage = AffinityStorage::new();
    _ = storage.bind_owner(current_thread.owner());

    for (thread, value) in values {
        if thread.owner() != current_thread.owner() {
            return Err(Error::ForeignOwner);
        }
        if storage.insert(S::key(&thread), Arc::into_std_arc(value)).is_err() {
            return Err(Error::Duplicate);
        }
    }

    let current = storage.get_clone(&S::key(current_thread)).ok_or(Error::MissingCurrent)?;

    Ok(AffinityState {
        shared: StdArc::new(AffinityShared {
            inner: RwLock::new(AffinityInner { storage, factory: None }),
        }),
        current,
        current_owner: Some(current_thread.owner().clone()),
    })
}

impl<T: ?Sized, S> Arc<T, S>
where
    S: Strategy<T>,
{
    fn from_state(state: S::State) -> Self {
        let this = Self {
            state,
            marker: PhantomData,
        };
        telemetry::record(EventKind::ArcCreate, Self::as_ptr(&this).cast::<()>());
        this
    }

    fn into_state(this: Self) -> S::State {
        let this = ManuallyDrop::new(this);
        // SAFETY: ManuallyDrop prevents the wrapper from dropping its state;
        // reading transfers ownership of that field to the caller.
        unsafe { std::ptr::read(&raw const this.state) }
    }

    /// Converts this pointer into the current standard-library Arc.
    #[must_use]
    pub fn into_std_arc(this: Self) -> StdArc<T> {
        S::into_current(Self::into_state(this))
    }

    /// Converts this pointer into the current standard-library Arc.
    #[must_use]
    pub fn into_arc(this: Self) -> StdArc<T> {
        Self::into_std_arc(this)
    }

    /// Returns the number of logical strong references to the current value.
    #[must_use]
    pub fn strong_count(this: &Self) -> usize {
        S::strong_count(&this.state)
    }

    /// Returns whether two pointers currently refer to the same allocation.
    #[must_use]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        StdArc::ptr_eq(S::current(&this.state), S::current(&other.state))
    }

    /// Returns a raw pointer to the current value.
    #[must_use]
    pub fn as_ptr(this: &Self) -> *const T {
        StdArc::as_ptr(S::current(&this.state))
    }

    /// Returns the current pointee address used as the telemetry identity.
    #[cfg(feature = "seismograph")]
    #[doc(hidden)]
    #[must_use]
    pub fn telemetry_object_id(this: &Self) -> seismograph::recorder::event::ObjectId {
        seismograph::recorder::event::ObjectId::from_ptr(Self::as_ptr(this).cast::<()>())
    }
}

impl<T: std::task::Wake> Arc<T, PerProcess> {
    /// Wakes the task represented by this pointer.
    pub fn wake(self) {
        std::task::Wake::wake(Self::into_std_arc(self));
    }

    /// Wakes the task without consuming this pointer.
    pub fn wake_by_ref(this: &Self) {
        std::task::Wake::wake_by_ref(&this.state);
    }
}

impl<T: std::task::Wake + Send + Sync + 'static> From<Arc<T, PerProcess>> for std::task::Waker {
    fn from(value: Arc<T, PerProcess>) -> Self {
        Self::from(Arc::into_std_arc(value))
    }
}

impl<T: ?Sized, S> Clone for Arc<T, S>
where
    S: Strategy<T>,
{
    fn clone(&self) -> Self {
        telemetry::record(EventKind::ArcClone, Self::as_ptr(self).cast::<()>());
        Self {
            state: S::clone_state(&self.state),
            marker: PhantomData,
        }
    }
}

impl<T: ?Sized, S> Deref for Arc<T, S>
where
    S: Strategy<T>,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        telemetry::record(EventKind::ArcDeref, Self::as_ptr(self).cast::<()>());
        S::current(&self.state)
    }
}

impl<T: ?Sized, S> Drop for Arc<T, S>
where
    S: Strategy<T>,
{
    fn drop(&mut self) {
        if S::strong_count(&self.state) == 1 {
            telemetry::record(EventKind::ArcDrop, Self::as_ptr(self).cast::<()>());
        }
    }
}

impl<T: ?Sized + fmt::Debug, S: Strategy<T>> fmt::Debug for Arc<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display, S: Strategy<T>> fmt::Display for Arc<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized + PartialEq, S: Strategy<T>> PartialEq for Arc<T, S> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: ?Sized + Eq, S: Strategy<T>> Eq for Arc<T, S> {}

impl<T: ?Sized + Hash, S: Strategy<T>> Hash for Arc<T, S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

impl<T: ?Sized + PartialOrd, S: Strategy<T>> PartialOrd for Arc<T, S> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: ?Sized + Ord, S: Strategy<T>> Ord for Arc<T, S> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized, S: Strategy<T>> AsRef<T> for Arc<T, S> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized, S: Strategy<T>> Borrow<T> for Arc<T, S> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized, S: Strategy<T>> fmt::Pointer for Arc<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&Self::as_ptr(self), f)
    }
}

impl<T: ?Sized> Default for Arc<T, PerProcess>
where
    StdArc<T>: Default,
{
    fn default() -> Self {
        Self::from_state(StdArc::default())
    }
}

impl<T> From<T> for Arc<T, PerProcess> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: ?Sized> From<Box<T>> for Arc<T, PerProcess> {
    fn from(value: Box<T>) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl<T> From<Vec<T>> for Arc<[T], PerProcess> {
    fn from(value: Vec<T>) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl<T, const N: usize> From<[T; N]> for Arc<[T], PerProcess> {
    fn from(value: [T; N]) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl<T: Clone> From<&[T]> for Arc<[T], PerProcess> {
    fn from(value: &[T]) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl<T> FromIterator<T> for Arc<[T], PerProcess> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl From<String> for Arc<str, PerProcess> {
    fn from(value: String) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl From<&str> for Arc<str, PerProcess> {
    fn from(value: &str) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl<'a> From<Cow<'a, str>> for Arc<str, PerProcess> {
    fn from(value: Cow<'a, str>) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl<'a, T: Clone> From<Cow<'a, [T]>> for Arc<[T], PerProcess> {
    fn from(value: Cow<'a, [T]>) -> Self {
        Self::from_state(StdArc::from(value))
    }
}

impl<T: ?Sized + Serialize, S: Strategy<T>> Serialize for Arc<T, S> {
    fn serialize<SerializerType>(&self, serializer: SerializerType) -> Result<SerializerType::Ok, SerializerType::Error>
    where
        SerializerType: Serializer,
    {
        (**self).serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for Arc<T, PerProcess>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::new)
    }
}

impl<'de, T> Deserialize<'de> for Arc<[T], PerProcess>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<T>::deserialize(deserializer).map(Self::from)
    }
}

impl<'de> Deserialize<'de> for Arc<str, PerProcess> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from)
    }
}

impl<T: ?Sized> Weak<T> {
    /// Attempts to create a strong reference.
    #[must_use]
    pub fn upgrade(&self) -> Option<Arc<T, PerProcess>> {
        self.inner.upgrade().map(Arc::from_state_unrecorded)
    }

    /// Returns the number of strong references to the allocation.
    #[must_use]
    pub fn strong_count(&self) -> usize {
        self.inner.strong_count()
    }

    /// Returns the number of weak references to the allocation.
    #[must_use]
    pub fn weak_count(&self) -> usize {
        self.inner.weak_count()
    }

    /// Returns whether two weak pointers refer to the same allocation.
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.inner.ptr_eq(&other.inner)
    }
}

impl<T> Weak<T> {
    /// Constructs an empty weak pointer.
    #[must_use]
    pub fn new() -> Self {
        Self { inner: StdWeak::new() }
    }
}

impl<T: ?Sized> Clone for Weak<T> {
    fn clone(&self) -> Self {
        Self {
            inner: StdWeak::clone(&self.inner),
        }
    }
}

impl<T> Default for Weak<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Weak<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::hash_map::DefaultHasher;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    use serde::de::value::{Error as ValueError, SeqDeserializer, StrDeserializer, U64Deserializer};
    use thread_aware::Relocator;

    use super::*;

    #[test]
    fn per_process_has_standard_arc_size() {
        assert_eq!(size_of::<Arc<u64, PerProcess>>(), size_of::<StdArc<u64>>());
    }

    #[test]
    fn per_process_clones_share_the_value() {
        let first = Arc::<_, PerProcess>::new(42);
        let second = first.clone();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(Arc::strong_count(&first), 2);
    }

    #[test]
    fn per_process_new_with_uses_the_constructor() {
        let value = Arc::<_, PerProcess>::new_with(|| 42);

        assert_eq!(*value, 42);
    }

    #[test]
    fn per_process_new_boxed_uses_the_constructor() {
        let value = Arc::<dyn fmt::Display + Send + Sync, PerProcess>::new_boxed(|| Box::new(42));

        assert_eq!(value.to_string(), "42");
    }

    #[test]
    fn per_process_new_with_data_uses_explicit_data() {
        let value = Arc::<_, PerProcess>::new_with_data(40, |value| value + 2);

        assert_eq!(*value, 42);
    }

    #[test]
    fn per_process_from_unaware_uses_the_value() {
        let value = Arc::<_, PerProcess>::from_unaware(String::from("value"));

        assert_eq!(&*value, "value");
    }

    #[test]
    fn per_process_with_clone_fn_uses_the_template() {
        let value = Arc::<u64, PerProcess>::with_clone_fn(42, |value| Box::new(*value));

        assert_eq!(*value, 42);
    }

    #[test]
    fn per_process_supports_unique_mutation_and_unwrap() {
        let mut value = Arc::new(String::from("initial"));
        Arc::get_mut(&mut value).unwrap().push_str("-updated");

        assert_eq!(Arc::try_unwrap(value), Ok(String::from("initial-updated")));
    }

    #[test]
    fn per_process_reports_failed_unwrap_and_clones_shared_values() {
        let value = Arc::new(String::from("shared"));
        let clone = value.clone();

        let value = Arc::try_unwrap(value).unwrap_err();

        assert!(Arc::ptr_eq(&value, &clone));
        assert_eq!(Arc::unwrap_or_clone(value), "shared");
    }

    #[test]
    fn per_process_wraps_standard_and_cyclic_arcs() {
        let standard = StdArc::new(42);
        let value = Arc::from_std(StdArc::clone(&standard));
        let cyclic = Arc::new_cyclic(Weak::strong_count);

        assert_eq!((StdArc::ptr_eq(&standard, &Arc::into_std_arc(value)), *cyclic), (true, 0));
    }

    #[test]
    fn per_process_pins_values() {
        let value = Arc::pin(42);

        assert_eq!(*value, 42);
    }

    #[test]
    fn per_process_weak_reference_tracks_lifetime() {
        let value = Arc::new(42);
        let weak = Arc::downgrade(&value);
        let upgraded = weak.upgrade().unwrap();
        drop(value);

        assert_eq!(Arc::into_inner(upgraded), Some(42));
        assert_eq!(weak.strong_count(), 0);
    }

    #[test]
    fn weak_references_support_counts_identity_defaults_and_formatting() {
        let value = Arc::new(42);
        let first = Arc::downgrade(&value);
        let second = first.clone();
        let empty = Weak::<u64>::default();

        assert_eq!(
            (
                Arc::weak_count(&value),
                first.weak_count(),
                first.ptr_eq(&second),
                empty.upgrade(),
                format!("{first:?}").is_empty(),
            ),
            (2, 2, true, None, false),
        );
    }

    #[test]
    fn per_process_supports_unsized_conversions() {
        let slice = Arc::<[u8]>::from(vec![1, 2, 3]);
        let text = Arc::<str>::from("value");

        assert_eq!((&*slice, &*text), (&[1, 2, 3][..], "value"));
    }

    #[test]
    fn per_process_slice_collects_from_iterator() {
        let values: Arc<[u8]> = [1, 2, 3].into_iter().collect();

        assert_eq!(&*values, &[1, 2, 3]);
    }

    #[test]
    fn per_process_supports_all_standard_conversions() {
        let default = Arc::<String>::default();
        let from_value: Arc<u64> = 42.into();
        let boxed: Box<dyn fmt::Display + Send + Sync> = Box::new(17);
        let from_box: Arc<dyn fmt::Display + Send + Sync> = boxed.into();
        let from_array: Arc<[u8]> = [1, 2, 3].into();
        let source = [4, 5, 6];
        let from_slice: Arc<[u8]> = source.as_slice().into();
        let from_string: Arc<str> = String::from("owned").into();
        let from_borrowed_cow: Arc<str> = Cow::Borrowed("borrowed").into();
        let from_owned_cow: Arc<[u8]> = Cow::<[u8]>::Owned(vec![7, 8]).into();

        assert_eq!(
            (
                &**default,
                *from_value,
                from_box.to_string(),
                &*from_array,
                &*from_slice,
                &*from_string,
                &*from_borrowed_cow,
                &*from_owned_cow,
            ),
            (
                "",
                42,
                "17".to_owned(),
                &[1, 2, 3][..],
                &[4, 5, 6][..],
                "owned",
                "borrowed",
                &[7, 8][..]
            ),
        );
    }

    #[test]
    fn per_process_make_mut_supports_sized_slice_and_str_values() {
        let mut sized = Arc::new(String::from("value"));
        let sized_clone = sized.clone();
        Arc::make_mut(&mut sized).push('!');

        let mut slice: Arc<[u8]> = [1, 2].into();
        let slice_clone = slice.clone();
        Arc::make_mut(&mut slice)[0] = 3;

        let mut text: Arc<str> = "lower".into();
        let text_clone = text.clone();
        Arc::make_mut(&mut text).make_ascii_uppercase();

        assert_eq!(
            (sized.as_str(), sized_clone.as_str(), &*slice, &*slice_clone, &*text, &*text_clone),
            ("value!", "value", &[3, 2][..], &[1, 2][..], "LOWER", "lower"),
        );
    }

    #[test]
    fn per_process_supports_uninitialized_allocations() {
        let mut uninitialized = Arc::<MaybeUninit<u64>>::new_uninit();
        Arc::get_mut(&mut uninitialized).unwrap().write(42);
        // SAFETY: the unique allocation was initialized immediately above.
        let initialized = unsafe { uninitialized.assume_init() };

        let zeroed = Arc::<MaybeUninit<u64>>::new_zeroed();
        // SAFETY: every bit pattern is valid for u64, including all zeroes.
        let zeroed = unsafe { zeroed.assume_init() };

        assert_eq!((*initialized, *zeroed), (42, 0));
    }

    #[test]
    fn per_process_supports_wake_and_waker_conversion() {
        struct Counter(AtomicUsize);

        impl Wake for Counter {
            fn wake(self: StdArc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        Arc::wake_by_ref(&counter);
        let waker = std::task::Waker::from(counter.clone());
        waker.wake_by_ref();
        counter.clone().wake();

        assert_eq!(counter.0.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn per_process_raw_pointer_round_trips() {
        let value = Arc::new(42);
        let pointer = Arc::into_raw(value);
        // SAFETY: pointer was produced by Arc::into_raw immediately above.
        let value = unsafe { Arc::from_raw(pointer) };

        assert_eq!(*value, 42);
    }

    #[test]
    fn per_process_raw_pointer_count_operations_preserve_lifetime() {
        let pointer = Arc::into_raw(Arc::new(42));
        // SAFETY: pointer represents a live Arc allocation.
        unsafe { Arc::increment_strong_count(pointer) };
        // SAFETY: the increment above created a second owned strong reference.
        let value = unsafe { Arc::from_raw(pointer) };
        // SAFETY: the original raw pointer still represents one owned strong reference.
        unsafe { Arc::decrement_strong_count(pointer) };

        assert_eq!(*value, 42);
    }

    #[test]
    fn per_process_downcasts_type_erased_values() {
        let value = Arc::new(42_u64).coerce::<dyn Any + Send + Sync>(|value| value);
        let value = value.downcast::<u64>().unwrap();

        assert_eq!(*value, 42);
    }

    #[test]
    fn per_process_failed_downcast_returns_the_original_value() {
        let value = Arc::new(42_u64).coerce::<dyn Any + Send + Sync>(|value| value);
        let value = value.downcast::<String>().unwrap_err();

        assert_eq!(*value.downcast::<u64>().unwrap(), 42);
    }

    #[test]
    fn per_process_traits_delegate_to_the_pointee() {
        let lower = Arc::new(7_u64);
        let higher = Arc::new(9_u64);
        let equal = Arc::new(7_u64);
        let mut arc_hasher = DefaultHasher::new();
        let mut value_hasher = DefaultHasher::new();
        lower.hash(&mut arc_hasher);
        7_u64.hash(&mut value_hasher);

        assert_eq!(
            (
                format!("{lower:?}"),
                format!("{lower}"),
                lower == equal,
                lower.partial_cmp(&higher),
                lower.cmp(&higher),
                *AsRef::<u64>::as_ref(&lower),
                *Borrow::<u64>::borrow(&lower),
                format!("{lower:p}").starts_with("0x"),
                arc_hasher.finish(),
            ),
            (
                "7".to_owned(),
                "7".to_owned(),
                true,
                Some(std::cmp::Ordering::Less),
                std::cmp::Ordering::Less,
                7,
                7,
                true,
                value_hasher.finish(),
            ),
        );
    }

    #[test]
    fn per_process_deserializes_sized_slice_and_str_values() {
        let decoded = Arc::<u64>::deserialize(U64Deserializer::<ValueError>::new(42)).unwrap();
        let slice = Arc::<[u8]>::deserialize(SeqDeserializer::<_, ValueError>::new([1_u8, 2, 3].into_iter())).unwrap();
        let text = Arc::<str>::deserialize(StrDeserializer::<ValueError>::new("value")).unwrap();

        assert_eq!((*decoded, &*slice, &*text), (42, &[1, 2, 3][..], "value"));
    }

    #[test]
    fn per_core_relocation_reuses_the_destination_value() {
        struct Counter(AtomicUsize);

        impl Counter {
            fn new() -> Self {
                Self(AtomicUsize::new(0))
            }
        }

        let relocator = Relocator::between_threads();
        let (source, destination) = relocator.relocate(&mut ());
        let source = source.unwrap();
        let mut first = Arc::<Counter, PerCore>::new_with(Counter::new);
        let mut second = first.clone();
        first.0.store(42, Ordering::Relaxed);

        ThreadAware::relocate(&mut second, Some(&source), &destination);
        second.0.store(7, Ordering::Relaxed);
        ThreadAware::relocate(&mut first, Some(&source), &destination);

        assert_eq!(first.0.load(Ordering::Relaxed), 7);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn per_core_adopts_prebuilt_values() {
        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let mut value =
            Arc::<u64, PerCore>::try_from_values(&source, [(source.clone(), Arc::new(10)), (destination.clone(), Arc::new(20))]).unwrap();

        assert_eq!(*value, 10);
        ThreadAware::relocate(&mut value, Some(&source), &destination);
        assert_eq!(*value, 20);
    }

    #[test]
    fn per_numa_materializes_and_adopts_region_values() {
        let (source, destination) = Relocator::between_numa_nodes().relocate(&mut ());
        let source = source.unwrap();
        let mut materialized = Arc::<u64, PerNuma>::new_with(|| 42);
        ThreadAware::relocate(&mut materialized, Some(&source), &destination);

        let mut prebuilt =
            Arc::<u64, PerNuma>::try_from_values(&source, [(source.clone(), Arc::new(10)), (destination.clone(), Arc::new(20))]).unwrap();
        ThreadAware::relocate(&mut prebuilt, Some(&source), &destination);

        assert_eq!((*materialized, *prebuilt), (42, 20));
    }

    #[test]
    fn prebuilt_values_reject_duplicate_partitions() {
        let (source, _) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let result = Arc::<u64, PerCore>::try_from_values(&source, [(source.clone(), Arc::new(10)), (source.clone(), Arc::new(20))]);

        assert_eq!(result.unwrap_err(), Error::Duplicate);
    }

    #[test]
    fn prebuilt_values_allow_sparse_partitions_and_reuse_the_current_value() {
        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let mut value = Arc::<u64, PerCore>::try_from_values(&source, [(source.clone(), Arc::new(10))]).unwrap();

        ThreadAware::relocate(&mut value, Some(&source), &destination);

        assert_eq!(*value, 10);
    }

    #[test]
    fn prebuilt_values_reject_missing_current_and_foreign_owners() {
        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let missing_current = Arc::<u64, PerCore>::try_from_values(&source, [(destination, Arc::new(20))]);

        let (source, foreign) = Relocator::between_threads().different_owner().relocate(&mut ());
        let source = source.unwrap();
        let foreign_owner = Arc::<u64, PerCore>::try_from_values(&source, [(source.clone(), Arc::new(10)), (foreign, Arc::new(30))]);

        assert_eq!(
            (missing_current.unwrap_err(), foreign_owner.unwrap_err()),
            (Error::MissingCurrent, Error::ForeignOwner)
        );
    }

    #[test]
    fn affinity_errors_describe_each_failure() {
        let errors = [Error::ForeignOwner, Error::Duplicate, Error::MissingCurrent];

        assert_eq!(
            errors.map(|error| error.to_string()),
            [
                "a supplied thread belongs to a different runtime owner",
                "multiple values were supplied for the same thread partition",
                "no value was supplied for the current thread partition",
            ],
        );
    }

    #[test]
    fn logical_count_excludes_storage_after_another_clone_relocates() {
        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let first = Arc::<u64, PerCore>::new_with(|| 42);
        let mut second = first.clone();

        ThreadAware::relocate(&mut second, Some(&source), &destination);

        assert_eq!(Arc::strong_count(&first), 1);
    }

    #[test]
    fn affinity_relocation_across_owners_keeps_the_carried_value() {
        let mut value = Arc::<u64, PerCore>::new_with(|| 42);
        let original = value.clone();

        _ = Relocator::between_threads().different_owner().relocate(&mut value);

        assert!(Arc::ptr_eq(&value, &original));
    }

    #[test]
    fn affinity_constructor_does_not_require_sync() {
        let value = Arc::<Cell<u64>, PerCore>::new_with(|| Cell::new(42));

        assert_eq!(value.get(), 42);
    }

    #[test]
    fn affinity_new_with_relocates_constructor_data() {
        #[derive(Clone)]
        struct Input(Option<std::thread::ThreadId>);

        impl ThreadAware for Input {
            fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
                self.0 = Some(destination.id());
            }
        }

        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let mut value = Arc::<Option<std::thread::ThreadId>, PerCore>::new_with_data(Input(None), |input| input.0);

        ThreadAware::relocate(&mut value, Some(&source), &destination);

        assert_eq!(*value, Some(destination.id()));
    }

    #[test]
    fn affinity_new_boxed_supports_unsized_values() {
        trait Value: Send + Sync {
            fn get(&self) -> usize;
        }

        impl Value for usize {
            fn get(&self) -> usize {
                *self
            }
        }

        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let mut value = Arc::<dyn Value, PerCore>::new_boxed(|| Box::new(42));

        ThreadAware::relocate(&mut value, Some(&source), &destination);

        assert_eq!(value.get(), 42);
    }

    #[test]
    fn affinity_from_unaware_clones_the_current_value() {
        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let mut value = Arc::<String, PerCore>::from_unaware(String::from("value"));
        let original = value.clone();

        ThreadAware::relocate(&mut value, Some(&source), &destination);

        assert_eq!(&*value, "value");
        assert!(!Arc::ptr_eq(&value, &original));
    }

    #[test]
    fn affinity_with_clone_fn_relocates_trait_objects() {
        trait Value: ThreadAware + Sync {
            fn get(&self) -> Option<std::thread::ThreadId>;
        }

        #[derive(Clone)]
        struct ConcreteValue(Option<std::thread::ThreadId>);

        impl ThreadAware for ConcreteValue {
            fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
                self.0 = Some(destination.id());
            }
        }

        impl Value for ConcreteValue {
            fn get(&self) -> Option<std::thread::ThreadId> {
                self.0
            }
        }

        let (source, destination) = Relocator::between_threads().relocate(&mut ());
        let source = source.unwrap();
        let mut value = Arc::<dyn Value, PerCore>::with_clone_fn(ConcreteValue(None), |value| Box::new(value.clone()));

        ThreadAware::relocate(&mut value, Some(&source), &destination);

        assert_eq!(value.get(), Some(destination.id()));
    }

    #[test]
    fn affinity_into_arc_returns_the_current_value() {
        let value = Arc::<u64, PerCore>::new_with(|| 42);

        assert_eq!(*Arc::into_arc(value), 42);
    }

    #[test]
    fn affinity_internal_state_and_factory_have_stable_debug_shapes() {
        let value = Arc::<u64, PerCore>::new_with(|| 42);
        let state_debug = format!("{:?}", value.state);
        let inner = value.state.shared.inner.read().unwrap();
        let factory_debug = format!("{:?}", inner.factory.as_ref().unwrap());

        assert_eq!(
            (state_debug, factory_debug),
            ("AffinityState { .. }".to_owned(), "Factory { source: None, .. }".to_owned())
        );
    }
}
