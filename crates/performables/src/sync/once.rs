// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::sync::OnceLock as StdOnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::telemetry::{self, EventKind};

/// A cell that can be initialized exactly once.
pub struct OnceLock<T> {
    inner: StdOnceLock<T>,
    initializing: AtomicBool,
}

impl<T> OnceLock<T> {
    /// Creates an empty cell.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: StdOnceLock::new(),
            initializing: AtomicBool::new(false),
        }
    }

    /// Returns the stored value, if initialized.
    pub fn get(&self) -> Option<&T> {
        let value = self.inner.get();
        if value.is_some() {
            self.record(EventKind::OnceAccess);
        }
        value
    }

    /// Returns mutable access to the stored value, if initialized.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.inner.get_mut()
    }

    /// Stores `value` if the cell is empty.
    ///
    /// # Errors
    ///
    /// Returns `value` when the cell has already been initialized.
    pub fn set(&self, value: T) -> Result<(), T> {
        let result = self.inner.set(value);
        if result.is_ok() {
            self.record(EventKind::OnceInitialize);
        }
        result
    }

    /// Returns the stored value, initializing it with `initialize` if needed.
    pub fn get_or_init<F>(&self, initialize: F) -> &T
    where
        F: FnOnce() -> T,
    {
        if self.inner.get().is_none() && self.initializing.load(Ordering::Acquire) {
            self.record(EventKind::OnceContention);
        }

        let value = self.inner.get_or_init(|| {
            self.initializing.store(true, Ordering::Release);
            let reset = InitializingReset(&self.initializing);
            let value = initialize();
            self.record(EventKind::OnceInitialize);
            drop(reset);
            value
        });
        self.record(EventKind::OnceAccess);
        value
    }

    /// Blocks until the cell is initialized and returns the stored value.
    pub fn wait(&self) -> &T {
        if self.inner.get().is_none() {
            self.record(EventKind::OnceContention);
        }
        let value = self.inner.wait();
        self.record(EventKind::OnceAccess);
        value
    }

    /// Takes the stored value, leaving the cell empty.
    pub fn take(&mut self) -> Option<T> {
        self.inner.take()
    }

    /// Consumes the cell and returns its stored value.
    pub fn into_inner(self) -> Option<T> {
        self.inner.into_inner()
    }

    fn peek(&self) -> Option<&T> {
        self.inner.get()
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Clone for OnceLock<T> {
    fn clone(&self) -> Self {
        self.inner.get().cloned().map_or_else(Self::new, Self::from)
    }
}

impl<T: fmt::Debug> fmt::Debug for OnceLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OnceLock").field(&self.inner).finish()
    }
}

impl<T: PartialEq> PartialEq for OnceLock<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: Eq> Eq for OnceLock<T> {}

impl<T> From<T> for OnceLock<T> {
    fn from(value: T) -> Self {
        Self {
            inner: StdOnceLock::from(value),
            initializing: AtomicBool::new(false),
        }
    }
}

struct InitializingReset<'a>(&'a AtomicBool);

impl Drop for InitializingReset<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// A lazily initialized value.
pub struct LazyLock<T, F = fn() -> T> {
    value: OnceLock<T>,
    initialize: UnsafeCell<Option<F>>,
}

// SAFETY: ownership of the initializer and initialized value can move between threads.
unsafe impl<T: Send, F: Send> Send for LazyLock<T, F> {}
// SAFETY: `OnceLock` serializes initialization and only the initializer accesses `initialize`.
unsafe impl<T: Sync, F: Send> Sync for LazyLock<T, F> {}

impl<T, F> LazyLock<T, F>
where
    F: FnOnce() -> T,
{
    /// Creates a lazily initialized value.
    #[must_use]
    pub const fn new(initialize: F) -> Self {
        Self {
            value: OnceLock::new(),
            initialize: UnsafeCell::new(Some(initialize)),
        }
    }

    /// Forces initialization and returns the value.
    ///
    /// # Panics
    ///
    /// Panics if an earlier initialization attempt panicked.
    pub fn force(this: &Self) -> &T {
        this.value.get_or_init(|| {
            // SAFETY: `OnceLock` invokes this closure exclusively.
            let initialize = unsafe { &mut *this.initialize.get() }
                .take()
                .expect("a panicking LazyLock initializer permanently consumes the initializer");
            initialize()
        })
    }
}

impl<T, F> Deref for LazyLock<T, F>
where
    F: FnOnce() -> T,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        Self::force(self)
    }
}

impl<T: fmt::Debug, F> fmt::Debug for LazyLock<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.value.peek() {
            Some(value) => f.debug_tuple("LazyLock").field(value).finish(),
            None => f.write_str("LazyLock(<uninitialized>)"),
        }
    }
}

impl<T: Default> Default for LazyLock<T> {
    fn default() -> Self {
        Self::new(T::default)
    }
}
