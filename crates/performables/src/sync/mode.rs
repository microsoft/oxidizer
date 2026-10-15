// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time selection of blocking or asynchronous synchronization.
//!
//! [`Sync`](crate::sync::mode::Sync) uses compact native blocking primitives.
//! [`Async`](crate::sync::mode::Async) also supports
//! executor-independent asynchronous waits on the same shared object.
//!
//! Specify the value type when using an otherwise unconstrained constructor:
//! `Mutex::<u64>::new(0)` selects the default blocking mode, while
//! `Mutex::<u64, Async>::new(0)` selects asynchronous support.
//! A type annotation such as `let mutex: Mutex<u64> = Mutex::new(0);` also
//! supplies the default. Without either, Rust cannot infer the strategy from
//! the protected value alone.

use super::{barrier, condition, lock, mutex};

/// Compact native synchronization with blocking and non-blocking operations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sync;

/// Executor-independent synchronization supporting both blocking and async waits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Async;

/// A sealed synchronization strategy implemented by [`Sync`] and [`Async`].
///
/// Associated storage is implementation plumbing, not an extension interface.
pub trait Mode: sealed::Sealed {
    /// Internal mutex state selected at compile time.
    #[doc(hidden)]
    type MutexState: Send + std::marker::Sync;
    /// Internal evidence of exclusive mutex ownership.
    #[doc(hidden)]
    type MutexGuard<'a>;
    /// Initial unlocked mutex state.
    #[doc(hidden)]
    const MUTEX_INIT: Self::MutexState;
    /// Internal reader-writer lock state selected at compile time.
    #[doc(hidden)]
    type RwLockState: Send + std::marker::Sync;
    /// Internal evidence of shared lock ownership.
    #[doc(hidden)]
    type RwLockReadGuard<'a>;
    /// Internal evidence of exclusive lock ownership.
    #[doc(hidden)]
    type RwLockWriteGuard<'a>;
    /// Initial unlocked reader-writer state.
    #[doc(hidden)]
    const RWLOCK_INIT: Self::RwLockState;
    /// Internal condition-variable state selected at compile time.
    #[doc(hidden)]
    type CondvarState: Send + std::marker::Sync + std::fmt::Debug;
    /// Initial condition-variable state.
    #[doc(hidden)]
    const CONDVAR_INIT: Self::CondvarState;
    /// Internal barrier state selected at compile time.
    #[doc(hidden)]
    type BarrierState: Send + std::marker::Sync + std::fmt::Debug;
}

#[expect(
    clippy::declare_interior_mutable_const,
    reason = "each use creates independent, initially unlocked ownership state"
)]
impl Mode for Sync {
    type MutexState = std::sync::Mutex<()>;
    type MutexGuard<'a> = Option<std::sync::MutexGuard<'a, ()>>;
    const MUTEX_INIT: Self::MutexState = std::sync::Mutex::new(());
    type RwLockState = std::sync::RwLock<()>;
    type RwLockReadGuard<'a> = Option<std::sync::RwLockReadGuard<'a, ()>>;
    type RwLockWriteGuard<'a> = Option<std::sync::RwLockWriteGuard<'a, ()>>;
    const RWLOCK_INIT: Self::RwLockState = std::sync::RwLock::new(());
    type CondvarState = std::sync::Condvar;
    const CONDVAR_INIT: Self::CondvarState = std::sync::Condvar::new();
    type BarrierState = barrier::StateSync;
}

#[expect(
    clippy::declare_interior_mutable_const,
    reason = "each use creates independent, initially unlocked ownership state"
)]
impl Mode for Async {
    type MutexState = mutex::StateAsync;
    type MutexGuard<'a> = ();
    const MUTEX_INIT: Self::MutexState = mutex::StateAsync::new();
    type RwLockState = lock::StateAsync;
    type RwLockReadGuard<'a> = ();
    type RwLockWriteGuard<'a> = ();
    const RWLOCK_INIT: Self::RwLockState = lock::StateAsync::new();
    type CondvarState = condition::StateAsync;
    const CONDVAR_INIT: Self::CondvarState = condition::StateAsync::new();
    type BarrierState = barrier::StateAsync;
}

// The private strategy is sealed because guard construction and release uphold
// UnsafeCell access invariants. Keeping initialization as associated constants
// permits one const constructor without ambiguous per-mode inherent methods.
pub(super) mod sealed {
    use super::super::PoisonError;
    use super::*;

    // Supertrait methods are callable through a public Mode bound even when
    // their trait's module is private. Releasing a still-live guard therefore
    // requires a capability that downstream safe code cannot construct.
    /// Capability authorizing internal guard destruction.
    ///
    /// A public mode bound must not allow releasing a still-live guard:
    ///
    /// ```compile_fail
    /// use performables::sync::{mode::Mode, mutex::MutexGuard};
    /// fn release_early<M: Mode>(guard: &mut MutexGuard<'_, u64, M>) {
    ///     M::mutex_release(guard);
    /// }
    /// ```
    #[derive(Debug)]
    pub struct Release {
        _private: (),
    }

    impl Release {
        pub(in crate::sync) const fn new() -> Self {
            Self { _private: () }
        }
    }

    #[expect(
        clippy::type_complexity,
        reason = "poison errors retain the same mode-specific guard as successful acquisition"
    )]
    pub trait Sealed: Sized {
        fn mutex_lock<T: ?Sized>(
            mutex: &mutex::Mutex<T, Self>,
        ) -> Result<mutex::MutexGuard<'_, T, Self>, PoisonError<mutex::MutexGuard<'_, T, Self>>>
        where
            Self: Mode;
        fn mutex_try_lock<T: ?Sized>(
            mutex: &mutex::Mutex<T, Self>,
        ) -> Result<Option<mutex::MutexGuard<'_, T, Self>>, PoisonError<mutex::MutexGuard<'_, T, Self>>>
        where
            Self: Mode;
        fn mutex_is_poisoned<T: ?Sized>(mutex: &mutex::Mutex<T, Self>) -> bool
        where
            Self: Mode;
        fn mutex_clear_poison<T: ?Sized>(mutex: &mutex::Mutex<T, Self>)
        where
            Self: Mode;
        fn mutex_release<T: ?Sized>(guard: &mut mutex::MutexGuard<'_, T, Self>, release: Release)
        where
            Self: Mode;

        fn rwlock_read<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<lock::RwLockReadGuard<'_, T, Self>, PoisonError<lock::RwLockReadGuard<'_, T, Self>>>
        where
            Self: Mode;
        fn rwlock_try_read<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<Option<lock::RwLockReadGuard<'_, T, Self>>, PoisonError<lock::RwLockReadGuard<'_, T, Self>>>
        where
            Self: Mode;
        fn rwlock_write<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<lock::RwLockWriteGuard<'_, T, Self>, PoisonError<lock::RwLockWriteGuard<'_, T, Self>>>
        where
            Self: Mode;
        fn rwlock_try_write<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<Option<lock::RwLockWriteGuard<'_, T, Self>>, PoisonError<lock::RwLockWriteGuard<'_, T, Self>>>
        where
            Self: Mode;
        fn rwlock_is_poisoned<T: ?Sized>(lock: &lock::RwLock<T, Self>) -> bool
        where
            Self: Mode;
        fn rwlock_clear_poison<T: ?Sized>(lock: &lock::RwLock<T, Self>)
        where
            Self: Mode;
        fn rwlock_release_read<T: ?Sized>(guard: &mut lock::RwLockReadGuard<'_, T, Self>, release: Release)
        where
            Self: Mode;
        fn rwlock_release_write<T: ?Sized>(guard: &mut lock::RwLockWriteGuard<'_, T, Self>, release: Release)
        where
            Self: Mode;

        fn barrier_new(parties: u32) -> <Self as Mode>::BarrierState
        where
            Self: Mode;
    }

    impl Sealed for Sync {
        fn mutex_lock<T: ?Sized>(
            mutex: &mutex::Mutex<T, Self>,
        ) -> Result<mutex::MutexGuard<'_, T, Self>, PoisonError<mutex::MutexGuard<'_, T, Self>>> {
            mutex.lock_result_native()
        }

        fn mutex_try_lock<T: ?Sized>(
            mutex: &mutex::Mutex<T, Self>,
        ) -> Result<Option<mutex::MutexGuard<'_, T, Self>>, PoisonError<mutex::MutexGuard<'_, T, Self>>> {
            mutex.try_lock_result_native()
        }

        fn mutex_is_poisoned<T: ?Sized>(mutex: &mutex::Mutex<T, Self>) -> bool {
            mutex.is_poisoned_native()
        }

        fn mutex_clear_poison<T: ?Sized>(mutex: &mutex::Mutex<T, Self>) {
            mutex.clear_poison_native();
        }

        fn mutex_release<T: ?Sized>(guard: &mut mutex::MutexGuard<'_, T, Self>, _release: Release) {
            guard.release_native();
        }

        fn rwlock_read<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<lock::RwLockReadGuard<'_, T, Self>, PoisonError<lock::RwLockReadGuard<'_, T, Self>>> {
            lock.read_result_native()
        }

        fn rwlock_try_read<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<Option<lock::RwLockReadGuard<'_, T, Self>>, PoisonError<lock::RwLockReadGuard<'_, T, Self>>> {
            lock.try_read_result_native()
        }

        fn rwlock_write<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<lock::RwLockWriteGuard<'_, T, Self>, PoisonError<lock::RwLockWriteGuard<'_, T, Self>>> {
            lock.write_result_native()
        }

        fn rwlock_try_write<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<Option<lock::RwLockWriteGuard<'_, T, Self>>, PoisonError<lock::RwLockWriteGuard<'_, T, Self>>> {
            lock.try_write_result_native()
        }

        fn rwlock_is_poisoned<T: ?Sized>(lock: &lock::RwLock<T, Self>) -> bool {
            lock.is_poisoned_native()
        }

        fn rwlock_clear_poison<T: ?Sized>(lock: &lock::RwLock<T, Self>) {
            lock.clear_poison_native();
        }

        fn rwlock_release_read<T: ?Sized>(guard: &mut lock::RwLockReadGuard<'_, T, Self>, _release: Release) {
            guard.release_native();
        }

        fn rwlock_release_write<T: ?Sized>(guard: &mut lock::RwLockWriteGuard<'_, T, Self>, _release: Release) {
            guard.release_native();
        }

        fn barrier_new(parties: u32) -> barrier::StateSync {
            barrier::StateSync::new(parties)
        }
    }

    impl Sealed for Async {
        fn mutex_lock<T: ?Sized>(
            mutex: &mutex::Mutex<T, Self>,
        ) -> Result<mutex::MutexGuard<'_, T, Self>, PoisonError<mutex::MutexGuard<'_, T, Self>>> {
            mutex.lock_result_inner()
        }

        fn mutex_try_lock<T: ?Sized>(
            mutex: &mutex::Mutex<T, Self>,
        ) -> Result<Option<mutex::MutexGuard<'_, T, Self>>, PoisonError<mutex::MutexGuard<'_, T, Self>>> {
            mutex.try_lock_result_inner()
        }

        fn mutex_is_poisoned<T: ?Sized>(mutex: &mutex::Mutex<T, Self>) -> bool {
            mutex.is_poisoned_inner()
        }

        fn mutex_clear_poison<T: ?Sized>(mutex: &mutex::Mutex<T, Self>) {
            mutex.clear_poison_inner();
        }

        fn mutex_release<T: ?Sized>(guard: &mut mutex::MutexGuard<'_, T, Self>, _release: Release) {
            guard.release_inner();
        }

        fn rwlock_read<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<lock::RwLockReadGuard<'_, T, Self>, PoisonError<lock::RwLockReadGuard<'_, T, Self>>> {
            lock.read_result_inner()
        }

        fn rwlock_try_read<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<Option<lock::RwLockReadGuard<'_, T, Self>>, PoisonError<lock::RwLockReadGuard<'_, T, Self>>> {
            lock.try_read_result_inner()
        }

        fn rwlock_write<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<lock::RwLockWriteGuard<'_, T, Self>, PoisonError<lock::RwLockWriteGuard<'_, T, Self>>> {
            lock.write_result_inner()
        }

        fn rwlock_try_write<T: ?Sized>(
            lock: &lock::RwLock<T, Self>,
        ) -> Result<Option<lock::RwLockWriteGuard<'_, T, Self>>, PoisonError<lock::RwLockWriteGuard<'_, T, Self>>> {
            lock.try_write_result_inner()
        }

        fn rwlock_is_poisoned<T: ?Sized>(lock: &lock::RwLock<T, Self>) -> bool {
            lock.is_poisoned_inner()
        }

        fn rwlock_clear_poison<T: ?Sized>(lock: &lock::RwLock<T, Self>) {
            lock.clear_poison_inner();
        }

        fn rwlock_release_read<T: ?Sized>(guard: &mut lock::RwLockReadGuard<'_, T, Self>, _release: Release) {
            guard.release_inner();
        }

        fn rwlock_release_write<T: ?Sized>(guard: &mut lock::RwLockWriteGuard<'_, T, Self>, _release: Release) {
            guard.release_inner();
        }

        fn barrier_new(parties: u32) -> barrier::StateAsync {
            barrier::StateAsync::new(parties)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::sync::barrier::Barrier;
    use crate::sync::condition::Condvar;
    use crate::sync::lock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
    use crate::sync::mutex::{Mutex, MutexGuard, MutexLock};

    #[test]
    fn annotations_select_the_default_constructor_mode() {
        let mutex: Mutex<u64> = Mutex::new(3);
        let lock: RwLock<u64> = RwLock::new(5);
        let condition: Condvar = Condvar::new();
        let barrier: Barrier = Barrier::new(1);

        let (guard, timeout) = condition.wait_timeout(mutex.lock(), std::time::Duration::ZERO);
        assert_eq!(
            (*guard, *lock.read(), timeout.timed_out(), barrier.wait().is_leader()),
            (3, 5, true, true)
        );
    }

    #[test]
    fn asynchronous_constructors_remain_const() {
        let mutex = const { Mutex::<u64, Async>::const_new(3) };
        let lock = const { RwLock::<u64, Async>::new(5) };
        let _condition = const { Condvar::<Async>::new() };

        assert_eq!((*mutex.lock(), *lock.read()), (3, 5));
    }

    #[test]
    fn mode_send_sync_contracts() {
        fn send<T: Send>() {}
        fn send_sync<T: Send + std::marker::Sync>() {}

        send_sync::<Mutex<Cell<u64>>>();
        send_sync::<Mutex<Cell<u64>, Async>>();
        send_sync::<RwLock<u64>>();
        send_sync::<RwLock<u64, Async>>();
        send::<MutexGuard<'_, Cell<u64>, Async>>();
        send::<MutexLock<'_, Cell<u64>>>();
        send_sync::<RwLockReadGuard<'_, u64, Async>>();
        send_sync::<RwLockWriteGuard<'_, u64, Async>>();
    }
}
