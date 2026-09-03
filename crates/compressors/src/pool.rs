// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reuse of compression engine state across codecs.

#[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
use std::collections::HashMap;
use std::fmt;
#[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd"))]
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};

#[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
use crate::flate::Wrapper;

/// How many idle engines the pool keeps per distinct configuration, unless told otherwise.
const DEFAULT_CAPACITY: usize = 16;

/// Identifies engines that are interchangeable with one another.
///
/// An engine can only be reused for the configuration it was built with: resetting a compressor
/// preserves its container and its level, so a gzip level-9 engine cannot serve a zlib level-1
/// request.
#[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EngineKey {
    pub(crate) wrapper: Wrapper,
    pub(crate) level: u8,
}

/// A shared, cloneable pool of reusable compression engine state.
///
/// Building a compressor allocates and initializes a substantial amount of state, and on a small
/// message that setup can cost as much as the compression itself. A service that builds a fresh
/// compressor per message therefore spends much of its compression budget getting ready to compress.
/// Recycling engines removes that cost.
///
/// The saving is roughly fixed per compressor, so it matters most for small messages and fades as
/// bodies grow -- which suits ordinary request and response traffic, where most bodies are small.
///
/// This is an implementation detail of [`Resources`][crate::Resources], which is how callers reach
/// it: a pool with a capacity of zero recycles nothing and gives them the baseline to measure
/// against.
///
/// # What is actually pooled
///
/// The pool is transparent: it recycles the engines that are worth recycling and silently builds
/// the rest, so calling code never has to know which is which. Measured, the engines it does not
/// pool are not worth pooling:
///
/// | Engine | Reused? |
/// |---|---|
/// | `deflate` / `zlib` / `gzip` compressor | yes -- `reset` preserves its container and level |
/// | `deflate` / `zlib` decompressor | yes -- `reset` restores the framing |
/// | `gzip` decompressor | no -- the underlying reset takes a boolean that cannot express gzip framing, so a recycled engine would silently decompress as raw deflate |
/// | `zstd` compressor and decompressor | yes -- `reset` keeps the context's allocations, which is where most of the cost is |
/// | `brotli` compressor and decompressor | no -- upstream exposes no reset, and recycling its buffers through a custom allocator was measured and did not pay for itself |
///
/// Decompressors are cheaper to build than compressors, but decompression is also much faster, so
/// the fixed setup cost is a comparable share of the work either way.
///
/// The gzip decompressor is the one gap worth explaining, because gzip is the encoding most often
/// seen on the wire. Nothing about gzip prevents recycling: the obstacle is only that the engine's
/// reset cannot express gzip framing. Taking over that framing here would let gzip decompressors join
/// the pool, but it would mean owning header parsing and checksum validation permanently in order
/// to route around an upstream API gap. That is a poor trade for a crate whose job is to stream
/// bytes, so the gap is left where it belongs. If the engine ever gains a reset that can express
/// gzip framing, gzip decompressors can start being pooled with no change to calling code.
///
/// Because this is an implementation detail rather than a contract, more engines can start being
/// pooled without any change to calling code.
///
/// # Bounds
///
/// The pool keeps at most [`Pool::capacity`] idle engines per distinct configuration, so a burst of
/// concurrent requests cannot make it grow without limit. Engines beyond that are dropped when they
/// are returned.
#[derive(Clone)]
pub(crate) struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    #[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
    compressors: Mutex<HashMap<EngineKey, Vec<flate2::Compress>>>,
    /// Decompressors carry no level, so the container alone identifies them.
    #[cfg(any(feature = "deflate", feature = "zlib"))]
    decompressors: Mutex<HashMap<Wrapper, Vec<flate2::Decompress>>>,
    /// Zstd contexts allocate their working memory lazily, so recycling them saves far more than
    /// their construction cost suggests.
    #[cfg(feature = "zstd")]
    zstd_compressors: Mutex<HashMap<i32, Vec<zstd_safe::CCtx<'static>>>>,
    #[cfg(feature = "zstd")]
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
    /// Size this to the number of messages you expect to be encoding at once. A capacity of zero
    /// disables recycling, which is useful for measuring what the pool is buying you.
    #[must_use]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                #[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
                compressors: Mutex::new(HashMap::new()),
                #[cfg(any(feature = "deflate", feature = "zlib"))]
                decompressors: Mutex::new(HashMap::new()),
                #[cfg(feature = "zstd")]
                zstd_compressors: Mutex::new(HashMap::new()),
                #[cfg(feature = "zstd")]
                zstd_decompressors: Mutex::new(Vec::new()),
                capacity,
            }),
        }
    }

    /// A shared pool that recycles nothing.
    ///
    /// Every API that builds a codec asks for a pool, so that going without recycling is a
    /// deliberate choice rather than the path of least resistance. This is that choice: one
    /// process-wide pool of capacity zero, so passing it costs no more than cloning a handle.
    ///
    /// Reach for it in tests, in one-off tools, and while measuring what a real pool is worth.
    /// Anything that compresses more than a handful of messages should hold a [`Pool::new`] instead.
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

    /// Whether this pool stores nothing, so that every operation on it can return without locking.
    ///
    /// A pool of capacity zero can neither hand an engine out nor keep one, so the locks it would
    /// take are pure overhead on a path this crate encourages callers to use.
    #[cfg_attr(
        not(any(feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")),
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
    /// The engine is reset before it is handed over, so a codec dropped part-way through a stream
    /// cannot leak its state into the next user.
    #[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
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
    #[cfg(any(feature = "deflate", feature = "gzip", feature = "zlib"))]
    pub(crate) fn return_compressor(&self, key: EngineKey, engine: &mut Option<flate2::Compress>) {
        if self.is_disabled() {
            return;
        }

        if let Ok(mut guard) = self.inner.compressors.lock() {
            let idle = guard.entry(key).or_default();
            if idle.len() < self.inner.capacity
                && let Some(engine) = engine.take()
            {
                idle.push(engine);
            }
        }
    }

    /// Takes an idle decompressor for `wrapper`, or reports that one must be built.
    ///
    /// Only called for containers whose reset restores the framing; see
    /// [`Wrapper::reset_restores_framing`].
    #[cfg(any(feature = "deflate", feature = "zlib"))]
    pub(crate) fn take_decompressor(&self, wrapper: Wrapper) -> Option<flate2::Decompress> {
        if self.is_disabled() {
            return None;
        }

        let mut engine = self.inner.decompressors.lock().ok()?.get_mut(&wrapper).and_then(Vec::pop)?;

        engine.reset(wrapper.expects_zlib_header());
        Some(engine)
    }

    /// Takes `engine` for reuse, leaving it in place when the pool cannot keep it; see [`Self::return_compressor`].
    #[cfg(any(feature = "deflate", feature = "zlib"))]
    pub(crate) fn return_decompressor(&self, wrapper: Wrapper, engine: &mut Option<flate2::Decompress>) {
        if self.is_disabled() {
            return;
        }

        if let Ok(mut guard) = self.inner.decompressors.lock() {
            let idle = guard.entry(wrapper).or_default();
            if idle.len() < self.inner.capacity
                && let Some(engine) = engine.take()
            {
                idle.push(engine);
            }
        }
    }

    /// Takes an idle zstd compressor built for `level`, or reports that one must be built.
    ///
    /// Resetting the session drops any half-written frame while keeping the context's allocations,
    /// which is where the saving comes from.
    #[cfg(feature = "zstd")]
    pub(crate) fn take_zstd_compressor(&self, level: i32) -> Option<zstd_safe::CCtx<'static>> {
        if self.is_disabled() {
            return None;
        }

        let mut context = self.inner.zstd_compressors.lock().ok()?.get_mut(&level).and_then(Vec::pop)?;

        context.reset(zstd_safe::ResetDirective::SessionAndParameters).ok()?;
        Some(context)
    }

    /// Takes `context` for reuse, leaving it in place when the pool cannot keep it, for the reason given on `return_compressor`.
    #[cfg(feature = "zstd")]
    pub(crate) fn return_zstd_compressor(&self, level: i32, context: &mut Option<zstd_safe::CCtx<'static>>) {
        if self.is_disabled() {
            return;
        }

        if let Ok(mut guard) = self.inner.zstd_compressors.lock() {
            let idle = guard.entry(level).or_default();
            if idle.len() < self.inner.capacity
                && let Some(context) = context.take()
            {
                idle.push(context);
            }
        }
    }

    /// Takes an idle zstd decompressor, or reports that one must be built.
    #[cfg(feature = "zstd")]
    pub(crate) fn take_zstd_decompressor(&self) -> Option<zstd_safe::DCtx<'static>> {
        if self.is_disabled() {
            return None;
        }

        let mut context = self.inner.zstd_decompressors.lock().ok()?.pop()?;

        context.reset(zstd_safe::ResetDirective::SessionAndParameters).ok()?;
        Some(context)
    }

    /// Takes `context` for reuse, leaving it in place when the pool cannot keep it, for the reason given on `return_compressor`.
    #[cfg(feature = "zstd")]
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

    #[cfg(feature = "gzip")]
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
            pool.inner
                .compressors
                .lock()
                .expect("pool is not poisoned")
                .get(&key)
                .map_or(0, Vec::len)
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
            dirty
                .compress(b"half a stream", &mut scratch, flate2::FlushCompress::None)
                .expect("compress");
            assert!(dirty.total_in() > 0, "the engine should be dirty");

            let pool = Pool::new();
            pool.return_compressor(key(6), &mut Some(dirty));

            let clean = pool.take_compressor(key(6)).expect("the engine comes back");
            assert_eq!(clean.total_in(), 0, "checkout must reset the engine");
            assert_eq!(clean.total_out(), 0);
        }

        #[test]
        fn a_poisoned_pool_silently_drops_a_returned_compressor() {
            let pool = Pool::new();

            // Poison the compressors mutex the same way a panicking holder would.
            let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = pool.inner.compressors.lock().expect("not yet poisoned");
                panic!("poisoning the mutex for the test");
            }));
            assert!(poisoned.is_err(), "the panic should have been caught");
            assert!(pool.inner.compressors.lock().is_err(), "the mutex must now be poisoned");

            // Recycling is an optimisation, so a poisoned pool must not panic the caller.
            pool.return_compressor(key(6), &mut Some(engine()));
            assert!(pool.take_compressor(key(6)).is_none(), "a poisoned pool has nothing to give");
        }
    }

    #[cfg(any(feature = "deflate", feature = "zlib"))]
    mod flate_decompressor_pooling {
        use super::*;

        #[cfg(feature = "deflate")]
        fn wrapper() -> Wrapper {
            Wrapper::Raw
        }

        #[cfg(all(feature = "zlib", not(feature = "deflate")))]
        fn wrapper() -> Wrapper {
            Wrapper::Zlib
        }

        fn engine() -> flate2::Decompress {
            wrapper().decompressor()
        }

        /// Counts what the pool is holding, which the public API deliberately does not expose.
        fn idle(pool: &Pool, wrapper: Wrapper) -> usize {
            pool.inner
                .decompressors
                .lock()
                .expect("pool is not poisoned")
                .get(&wrapper)
                .map_or(0, Vec::len)
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
                let _guard = pool.inner.decompressors.lock().expect("not yet poisoned");
                panic!("poisoning the mutex for the test");
            }));
            assert!(poisoned.is_err(), "the panic should have been caught");
            assert!(pool.inner.decompressors.lock().is_err(), "the mutex must now be poisoned");

            pool.return_decompressor(wrapper(), &mut Some(engine()));
            assert!(pool.take_decompressor(wrapper()).is_none(), "a poisoned pool has nothing to give");
        }
    }

    #[cfg(feature = "zstd")]
    mod zstd_pooling {
        use super::*;

        /// Counts what the pool is holding, which the public API deliberately does not expose.
        fn idle_compressors(pool: &Pool, level: i32) -> usize {
            pool.inner
                .zstd_compressors
                .lock()
                .expect("pool is not poisoned")
                .get(&level)
                .map_or(0, Vec::len)
        }

        fn idle_decompressors(pool: &Pool) -> usize {
            pool.inner.zstd_decompressors.lock().expect("pool is not poisoned").len()
        }

        #[test]
        fn a_compressor_survives_a_round_trip_through_the_pool() {
            let pool = Pool::new();
            assert!(pool.take_zstd_compressor(3).is_none(), "an empty pool has nothing to give");

            pool.return_zstd_compressor(3, &mut Some(zstd_safe::CCtx::create()));
            assert_eq!(idle_compressors(&pool, 3), 1);

            assert!(pool.take_zstd_compressor(3).is_some(), "the returned engine should come back");
            assert_eq!(idle_compressors(&pool, 3), 0, "taking an engine removes it from the pool");
        }

        #[test]
        fn compressor_capacity_bounds_what_is_retained() {
            let pool = Pool::with_capacity(2);
            for _ in 0..5 {
                pool.return_zstd_compressor(3, &mut Some(zstd_safe::CCtx::create()));
            }

            assert_eq!(idle_compressors(&pool, 3), 2, "only `capacity` engines are kept");
        }

        #[test]
        fn zero_capacity_disables_zstd_compressor_recycling() {
            let pool = Pool::with_capacity(0);
            pool.return_zstd_compressor(3, &mut Some(zstd_safe::CCtx::create()));

            assert!(pool.take_zstd_compressor(3).is_none());
        }

        #[test]
        fn a_poisoned_pool_silently_drops_a_returned_zstd_compressor() {
            let pool = Pool::new();

            let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = pool.inner.zstd_compressors.lock().expect("not yet poisoned");
                panic!("poisoning the mutex for the test");
            }));
            assert!(poisoned.is_err(), "the panic should have been caught");
            assert!(pool.inner.zstd_compressors.lock().is_err(), "the mutex must now be poisoned");

            pool.return_zstd_compressor(3, &mut Some(zstd_safe::CCtx::create()));
            assert!(pool.take_zstd_compressor(3).is_none(), "a poisoned pool has nothing to give");
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
