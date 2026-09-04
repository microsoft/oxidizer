// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reuse of compression engine state from one stream to the next.

#[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
use std::collections::HashMap;
use std::fmt;
#[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};

#[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
use crate::flate::Wrapper;

/// How many idle engines the pool keeps per interchangeable group, unless told otherwise.
///
/// Sized for a service handling a moderate number of concurrent messages: high enough that an
/// ordinary request burst is served from the pool rather than rebuilding, low enough that idle
/// engine state is not retained indefinitely for a workload that has gone quiet. A conservative
/// starting point rather than a measured optimum -- a caller who knows its concurrency should say
/// so with [`Resources::with_pool_capacity`][crate::Resources::with_pool_capacity].
const DEFAULT_CAPACITY: usize = 16;

/// Identifies engines that are interchangeable with one another.
///
/// An engine can only be reused for the configuration it was built with: resetting a compressor
/// preserves its container and its level, so a gzip level-9 engine cannot serve a zlib level-1
/// request.
#[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EngineKey {
    pub(crate) wrapper: Wrapper,
    pub(crate) level: u8,
}

/// A shared, cloneable pool of reusable compression engine state.
///
/// Building an engine allocates and initializes a substantial amount of state -- on a small message,
/// as much work as the compression itself -- so recycling removes a cost that is roughly fixed per
/// engine and therefore matters most for small messages.
///
/// Reached through [`Resources`][crate::Resources]; a capacity of zero recycles nothing.
///
/// # What is pooled
///
/// | Engine | Reused? |
/// |---|---|
/// | `deflate` / `zlib` / `gzip` compressor | yes -- `reset` preserves its container and level |
/// | `deflate` / `zlib` decompressor | yes -- `reset` restores the framing |
/// | `gzip` decompressor | no -- see below |
/// | `zstd` compressor and decompressor | yes -- `reset` keeps the context's allocations, which is where most of the cost is |
/// | `brotli` compressor and decompressor | no -- upstream exposes no reset, and recycling its buffers through a custom allocator was measured and did not pay for itself |
///
/// The gzip decompressor is the gap worth explaining. `flate2`'s reset takes a boolean that cannot
/// express gzip framing, so a recycled engine would silently decompress as raw deflate. Taking that
/// framing over here would let gzip decompressors join the pool, but only by owning header parsing
/// and checksum validation permanently to route around an upstream API gap. If `flate2` gains a
/// reset that can express gzip framing, they can start being pooled with no change elsewhere.
///
/// # Bounds
///
/// At most [`Pool::capacity`] idle engines per distinct configuration, so a burst of concurrent
/// requests cannot make it grow without limit. Engines beyond that are dropped when returned.
#[derive(Clone)]
pub(crate) struct Pool {
    inner: Arc<Inner>,
}

/// The shared state every [`Pool`] clone points at.
///
/// A `Pool` is a handle: cloning one shares this, which is what lets a `Resources` be handed around
/// while every compressor and decompressor built from it draws on the same idle engines.
///
/// Each engine class gets its own [`Mutex`] rather than one lock over everything, so a compressor
/// being returned never waits on a decompressor being taken, and poisoning is contained to the one
/// class whose critical section panicked -- every checkout treats a poisoned lock as "nothing to
/// reuse" and builds a fresh engine instead.
struct Inner {
    #[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
    compressors: Mutex<HashMap<EngineKey, Vec<flate2::Compress>>>,
    /// Decompressors carry no level, so the container alone identifies them.
    #[cfg(any(test, feature = "deflate", feature = "zlib"))]
    decompressors: Mutex<HashMap<Wrapper, Vec<flate2::Decompress>>>,
    /// Zstd contexts allocate their working memory lazily, so recycling them saves far more than
    /// their construction cost suggests.
    ///
    /// Not keyed by level: checkout resets with `SessionAndParameters` and the compressor then
    /// applies its level unconditionally, so any idle context serves any level. Keying by level
    /// would only fragment reuse and let a caller-chosen level grow the map.
    #[cfg(any(test, feature = "zstd"))]
    zstd_compressors: Mutex<Vec<zstd_safe::CCtx<'static>>>,
    #[cfg(any(test, feature = "zstd"))]
    zstd_decompressors: Mutex<Vec<zstd_safe::DCtx<'static>>>,
    capacity: usize,
}

impl Pool {
    /// Creates a pool that keeps up to 16 idle engines per configuration.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Creates a pool that keeps up to `capacity` idle engines per configuration.
    ///
    /// Sized to the number of messages expected to be in flight at once. A capacity of zero
    /// disables recycling.
    #[must_use]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                #[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
                compressors: Mutex::new(HashMap::new()),
                #[cfg(any(test, feature = "deflate", feature = "zlib"))]
                decompressors: Mutex::new(HashMap::new()),
                #[cfg(any(test, feature = "zstd"))]
                zstd_compressors: Mutex::new(Vec::new()),
                #[cfg(any(test, feature = "zstd"))]
                zstd_decompressors: Mutex::new(Vec::new()),
                capacity,
            }),
        }
    }

    /// A shared pool that recycles nothing.
    ///
    /// Every API that builds an engine asks for a pool, so going without recycling has to be an
    /// explicit choice. This is that choice: one process-wide pool of capacity zero, so passing it
    /// costs no more than cloning a handle.
    #[must_use]
    pub(crate) fn disabled() -> &'static Self {
        static DISABLED: OnceLock<Pool> = OnceLock::new();

        DISABLED.get_or_init(|| Self::with_capacity(0))
    }

    /// The most idle engines this pool keeps per distinct configuration.
    #[must_use]
    pub(crate) fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Whether this pool stores nothing, so that every access can return without locking.
    ///
    /// A pool of capacity zero can neither hand an engine out nor keep one, so the locks it would
    /// take are pure overhead.
    #[cfg_attr(
        not(any(test, feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")),
        expect(dead_code, reason = "only the pooled formats ask, and none of them is enabled")
    )]
    // Answering `false` here is unobservable: every caller then takes the lock and reaches a
    // capacity check that a pool of zero fails anyway, handing back the same engine and keeping
    // the same nothing. Only the lock traffic differs, so no test can hold this to account.
    #[cfg_attr(test, mutants::skip)]
    fn is_disabled(&self) -> bool {
        self.capacity() == 0
    }

    /// Takes an idle compressor for `key`, or reports that one must be built.
    ///
    /// The engine is reset before it is handed over, so an engine dropped part-way through a stream
    /// cannot leak its state into the next user.
    #[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
    pub(crate) fn take_compressor(&self, key: EngineKey) -> Option<flate2::Compress> {
        if self.is_disabled() {
            return None;
        }

        // A poisoned pool is not worth propagating: recycling is an optimisation, so building a
        // fresh engine is always preferable to failing the caller's compression.
        let mut engine = self.inner.compressors.lock().ok()?.get_mut(&key).and_then(Vec::pop)?;

        engine.reset();
        Some(engine)
    }

    /// Takes `engine` for reuse, leaving it in place when the pool cannot keep it.
    ///
    /// The engine is borrowed rather than consumed so that a pool which will not store it never
    /// takes ownership. Every caller is a [`Drop`] implementation, and dropping the engine there
    /// would free it while the value being destroyed is still borrowed, which the aliasing rules
    /// forbid.
    #[cfg(any(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
    pub(crate) fn return_compressor(&self, key: EngineKey, engine: &mut Option<flate2::Compress>) {
        if self.is_disabled() {
            return;
        }

        if let Ok(mut guard) = self.inner.compressors.lock() {
            // Probe before inserting. `entry(..).or_default()` leaves an empty bucket behind for
            // every key it is asked about, so a full pool would still grow the map without bound.
            if guard.get(&key).map_or(0, Vec::len) < self.inner.capacity
                && let Some(engine) = engine.take()
            {
                guard.entry(key).or_default().push(engine);
            }
        }
    }

    /// Takes an idle decompressor for `wrapper`, or reports that one must be built.
    ///
    /// Only called for containers whose reset restores the framing; see
    /// [`Wrapper::reset_restores_framing`].
    #[cfg(any(test, feature = "deflate", feature = "zlib"))]
    pub(crate) fn take_decompressor(&self, wrapper: Wrapper) -> Option<flate2::Decompress> {
        if self.is_disabled() {
            return None;
        }

        let mut engine = self.inner.decompressors.lock().ok()?.get_mut(&wrapper).and_then(Vec::pop)?;

        engine.reset(wrapper.expects_zlib_header());
        Some(engine)
    }

    /// Takes `engine` for reuse, leaving it in place when the pool cannot keep it; see [`Self::return_compressor`].
    #[cfg(any(test, feature = "deflate", feature = "zlib"))]
    pub(crate) fn return_decompressor(&self, wrapper: Wrapper, engine: &mut Option<flate2::Decompress>) {
        if self.is_disabled() {
            return;
        }

        if let Ok(mut guard) = self.inner.decompressors.lock() {
            // Probe before inserting, for the reason given on `return_compressor`.
            if guard.get(&wrapper).map_or(0, Vec::len) < self.inner.capacity
                && let Some(engine) = engine.take()
            {
                guard.entry(wrapper).or_default().push(engine);
            }
        }
    }

    /// Takes an idle zstd compressor built for `level`, or reports that one must be built.
    ///
    /// Resetting the session drops any half-written frame while keeping the context's allocations,
    /// which is where the saving comes from.
    #[cfg(any(test, feature = "zstd"))]
    pub(crate) fn take_zstd_compressor(&self) -> Option<zstd_safe::CCtx<'static>> {
        if self.is_disabled() {
            return None;
        }

        let mut context = self.inner.zstd_compressors.lock().ok()?.pop()?;

        context.reset(zstd_safe::ResetDirective::SessionAndParameters).ok()?;
        Some(context)
    }

    /// Takes `context` for reuse, leaving it in place when the pool cannot keep it, for the reason given on `return_compressor`.
    #[cfg(any(test, feature = "zstd"))]
    pub(crate) fn return_zstd_compressor(&self, context: &mut Option<zstd_safe::CCtx<'static>>) {
        if self.is_disabled() {
            return;
        }

        if let Ok(mut guard) = self.inner.zstd_compressors.lock()
            && guard.len() < self.inner.capacity
            && let Some(context) = context.take()
        {
            guard.push(context);
        }
    }

    /// Takes an idle zstd decompressor, or reports that one must be built.
    #[cfg(any(test, feature = "zstd"))]
    pub(crate) fn take_zstd_decompressor(&self) -> Option<zstd_safe::DCtx<'static>> {
        if self.is_disabled() {
            return None;
        }

        let mut context = self.inner.zstd_decompressors.lock().ok()?.pop()?;

        context.reset(zstd_safe::ResetDirective::SessionAndParameters).ok()?;
        Some(context)
    }

    /// Takes `context` for reuse, leaving it in place when the pool cannot keep it, for the reason given on `return_compressor`.
    #[cfg(any(test, feature = "zstd"))]
    pub(crate) fn return_zstd_decompressor(&self, context: &mut Option<zstd_safe::DCtx<'static>>) {
        if self.is_disabled() {
            return;
        }

        if let Ok(mut guard) = self.inner.zstd_decompressors.lock()
            && guard.len() < self.inner.capacity
            && let Some(context) = context.take()
        {
            guard.push(context);
        }
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("capacity", &self.inner.capacity)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capacity_is_applied() {
        assert_eq!(Pool::new().capacity(), DEFAULT_CAPACITY);
        assert_eq!(Pool::default().capacity(), DEFAULT_CAPACITY);
        assert_eq!(Pool::with_capacity(3).capacity(), 3);
    }

    #[test]
    fn clones_share_one_pool() {
        let pool = Pool::new();
        let clone = pool.clone();

        assert!(Arc::ptr_eq(&pool.inner, &clone.inner), "cloning must not fork the pool");
    }

    #[test]
    fn debug_reports_capacity() {
        assert!(format!("{:?}", Pool::with_capacity(4)).contains("capacity: 4"));
    }

    #[cfg(any(test, feature = "gzip"))]
    mod pooling {
        use super::*;
        use crate::Level;

        fn key(level: u8) -> EngineKey {
            EngineKey {
                wrapper: Wrapper::Gzip,
                level,
            }
        }

        fn engine() -> flate2::Compress {
            Wrapper::Gzip.compressor(Level::DEFAULT)
        }

        /// Counts what the pool is holding, which the public API deliberately does not expose.
        fn idle(pool: &Pool, key: EngineKey) -> usize {
            pool.inner.compressors.lock().unwrap().get(&key).map_or(0, Vec::len)
        }

        #[test]
        fn an_engine_survives_a_round_trip_through_the_pool() {
            let pool = Pool::new();
            assert!(pool.take_compressor(key(6)).is_none(), "an empty pool has nothing to give");

            pool.return_compressor(key(6), &mut Some(engine()));
            assert_eq!(idle(&pool, key(6)), 1);

            assert!(pool.take_compressor(key(6)).is_some(), "the returned engine should come back");
            assert_eq!(idle(&pool, key(6)), 0, "taking an engine removes it from the pool");
        }

        #[test]
        fn engines_are_not_shared_between_configurations() {
            let pool = Pool::new();
            pool.return_compressor(key(6), &mut Some(engine()));

            assert!(
                pool.take_compressor(key(9)).is_none(),
                "a level-9 request must not receive a level-6 engine"
            );
        }

        #[test]
        fn capacity_bounds_what_is_retained() {
            let pool = Pool::with_capacity(2);
            for _ in 0..5 {
                pool.return_compressor(key(6), &mut Some(engine()));
            }

            assert_eq!(idle(&pool, key(6)), 2, "only `capacity` engines are kept");
        }

        #[test]
        fn zero_capacity_disables_recycling() {
            let pool = Pool::with_capacity(0);
            pool.return_compressor(key(6), &mut Some(engine()));

            assert_eq!(idle(&pool, key(6)), 0);
            assert!(pool.take_compressor(key(6)).is_none());
        }

        #[test]
        fn a_returned_engine_is_reset_before_reuse() {
            // An engine abandoned mid-stream must not leak its state into the next user.
            let mut dirty = engine();
            let mut scratch = [0_u8; 256];
            dirty.compress(b"half a stream", &mut scratch, flate2::FlushCompress::None).unwrap();
            assert!(dirty.total_in() > 0, "the engine should be dirty");

            let pool = Pool::new();
            pool.return_compressor(key(6), &mut Some(dirty));

            let clean = pool.take_compressor(key(6)).unwrap();
            assert_eq!(clean.total_in(), 0, "checkout must reset the engine");
            assert_eq!(clean.total_out(), 0);
        }

        #[test]
        fn a_poisoned_pool_silently_drops_a_returned_compressor() {
            let pool = Pool::new();

            // Poison the compressors mutex the same way a panicking holder would.
            let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = pool.inner.compressors.lock().unwrap();
                panic!("poisoning the mutex for the test");
            }));
            assert!(poisoned.is_err(), "the panic should have been caught");
            assert!(pool.inner.compressors.lock().is_err(), "the mutex must now be poisoned");

            // Recycling is an optimisation, so a poisoned pool must not panic the caller.
            pool.return_compressor(key(6), &mut Some(engine()));
            assert!(pool.take_compressor(key(6)).is_none(), "a poisoned pool has nothing to give");
        }
    }

    #[cfg(any(test, feature = "deflate", feature = "zlib"))]
    mod flate_decompressor_pooling {
        use super::*;

        #[cfg(any(test, feature = "deflate"))]
        fn wrapper() -> Wrapper {
            Wrapper::Raw
        }

        #[cfg(all(not(test), feature = "zlib", not(feature = "deflate")))]
        fn wrapper() -> Wrapper {
            Wrapper::Zlib
        }

        fn engine() -> flate2::Decompress {
            wrapper().decompressor()
        }

        /// Counts what the pool is holding, which the public API deliberately does not expose.
        fn idle(pool: &Pool, wrapper: Wrapper) -> usize {
            pool.inner.decompressors.lock().unwrap().get(&wrapper).map_or(0, Vec::len)
        }

        #[test]
        fn an_engine_survives_a_round_trip_through_the_pool() {
            let pool = Pool::new();
            assert!(pool.take_decompressor(wrapper()).is_none(), "an empty pool has nothing to give");

            pool.return_decompressor(wrapper(), &mut Some(engine()));
            assert_eq!(idle(&pool, wrapper()), 1);

            assert!(pool.take_decompressor(wrapper()).is_some(), "the returned engine should come back");
            assert_eq!(idle(&pool, wrapper()), 0, "taking an engine removes it from the pool");
        }

        #[test]
        fn capacity_bounds_what_is_retained() {
            let pool = Pool::with_capacity(2);
            for _ in 0..5 {
                pool.return_decompressor(wrapper(), &mut Some(engine()));
            }

            assert_eq!(idle(&pool, wrapper()), 2, "only `capacity` engines are kept");
        }

        #[test]
        fn zero_capacity_disables_decompressor_recycling() {
            let pool = Pool::with_capacity(0);
            pool.return_decompressor(wrapper(), &mut Some(engine()));

            assert!(pool.take_decompressor(wrapper()).is_none());
        }

        #[test]
        fn a_poisoned_pool_silently_drops_a_returned_decompressor() {
            let pool = Pool::new();

            let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = pool.inner.decompressors.lock().unwrap();
                panic!("poisoning the mutex for the test");
            }));
            assert!(poisoned.is_err(), "the panic should have been caught");
            assert!(pool.inner.decompressors.lock().is_err(), "the mutex must now be poisoned");

            pool.return_decompressor(wrapper(), &mut Some(engine()));
            assert!(pool.take_decompressor(wrapper()).is_none(), "a poisoned pool has nothing to give");
        }
    }

    #[cfg(any(test, feature = "zstd"))]
    mod zstd_pooling {
        use super::*;

        /// Counts what the pool is holding, which the public API deliberately does not expose.
        fn idle_compressors(pool: &Pool) -> usize {
            pool.inner.zstd_compressors.lock().unwrap().len()
        }

        fn idle_decompressors(pool: &Pool) -> usize {
            pool.inner.zstd_decompressors.lock().unwrap().len()
        }

        #[test]
        fn a_compressor_survives_a_round_trip_through_the_pool() {
            let pool = Pool::new();
            assert!(pool.take_zstd_compressor().is_none(), "an empty pool has nothing to give");

            pool.return_zstd_compressor(&mut Some(zstd_safe::CCtx::create()));
            assert_eq!(idle_compressors(&pool), 1);

            assert!(pool.take_zstd_compressor().is_some(), "the returned engine should come back");
            assert_eq!(idle_compressors(&pool), 0, "taking an engine removes it from the pool");
        }

        #[test]
        fn compressor_capacity_bounds_what_is_retained() {
            let pool = Pool::with_capacity(2);
            for _ in 0..5 {
                pool.return_zstd_compressor(&mut Some(zstd_safe::CCtx::create()));
            }

            assert_eq!(idle_compressors(&pool), 2, "only `capacity` engines are kept");
        }

        #[test]
        fn zero_capacity_disables_zstd_compressor_recycling() {
            let pool = Pool::with_capacity(0);
            pool.return_zstd_compressor(&mut Some(zstd_safe::CCtx::create()));

            assert!(pool.take_zstd_compressor().is_none());
        }

        #[test]
        fn a_poisoned_pool_silently_drops_a_returned_zstd_compressor() {
            let pool = Pool::new();

            let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = pool.inner.zstd_compressors.lock().unwrap();
                panic!("poisoning the mutex for the test");
            }));
            assert!(poisoned.is_err(), "the panic should have been caught");
            assert!(pool.inner.zstd_compressors.lock().is_err(), "the mutex must now be poisoned");

            pool.return_zstd_compressor(&mut Some(zstd_safe::CCtx::create()));
            assert!(pool.take_zstd_compressor().is_none(), "a poisoned pool has nothing to give");
        }

        #[test]
        fn a_decompressor_survives_a_round_trip_through_the_pool() {
            let pool = Pool::new();
            assert!(pool.take_zstd_decompressor().is_none(), "an empty pool has nothing to give");

            pool.return_zstd_decompressor(&mut Some(zstd_safe::DCtx::create()));
            assert_eq!(idle_decompressors(&pool), 1);

            assert!(pool.take_zstd_decompressor().is_some(), "the returned engine should come back");
            assert_eq!(idle_decompressors(&pool), 0, "taking an engine removes it from the pool");
        }

        #[test]
        fn decompressor_capacity_bounds_what_is_retained() {
            let pool = Pool::with_capacity(2);
            for _ in 0..5 {
                pool.return_zstd_decompressor(&mut Some(zstd_safe::DCtx::create()));
            }

            assert_eq!(idle_decompressors(&pool), 2, "only `capacity` engines are kept");
        }

        #[test]
        fn zero_capacity_disables_zstd_decompressor_recycling() {
            let pool = Pool::with_capacity(0);
            pool.return_zstd_decompressor(&mut Some(zstd_safe::DCtx::create()));

            assert!(pool.take_zstd_decompressor().is_none());
        }
    }
}
