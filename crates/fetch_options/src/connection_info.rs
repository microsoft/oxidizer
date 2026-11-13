// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Information about connections that served HTTP responses.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::PoolIndex;

/// Information about the connection that served an HTTP response.
///
/// A transport that supports this information attaches it as a response extension.
/// Read it with `response.extensions().get::<ConnectionInfo>()`. Clones share the
/// same state.
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    inner: Arc<ConnectionInfoInner>,
}

#[derive(Debug)]
struct ConnectionInfoInner {
    start: Instant,
    now: NowFn,
    pool_index: PoolIndex,
    max_age: Option<Duration>,
    poisoned: AtomicBool,
}

/// A boxed clock function used to measure connection age.
struct NowFn(Box<dyn Fn() -> Instant + Send + Sync + 'static>);

impl NowFn {
    fn now(&self) -> Instant {
        (self.0)()
    }
}

impl fmt::Debug for NowFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NowFn").field(&format_args!("<closure>")).finish()
    }
}

impl ConnectionInfo {
    /// Creates connection information that uses the supplied clock to measure age.
    ///
    /// This function calls `now` once to record the start time. Each [`age`](Self::age)
    /// call invokes `now` again. A test clock gives deterministic age values.
    #[must_use]
    pub fn new<F>(now: F, pool_index: PoolIndex, max_age: Option<Duration>) -> Self
    where
        F: Fn() -> Instant + Send + Sync + 'static,
    {
        let start = now();
        Self {
            inner: Arc::new(ConnectionInfoInner {
                start,
                now: NowFn(Box::new(now)),
                pool_index,
                max_age,
                poisoned: AtomicBool::new(false),
            }),
        }
    }

    /// Time elapsed since the connection was established.
    #[must_use]
    pub fn age(&self) -> Duration {
        self.inner.now.now().saturating_duration_since(self.inner.start)
    }

    /// Returns the pool that served the request.
    ///
    /// A client with one pool always returns `PoolIndex::new(0)`.
    #[must_use]
    pub fn pool_index(&self) -> PoolIndex {
        self.inner.pool_index
    }

    /// Returns `true` after [`poison`](Self::poison) marks the connection for removal.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.inner.poisoned.load(Ordering::Relaxed)
    }

    /// Marks the connection for removal from the pool.
    ///
    /// Call this associated function as `ConnectionInfo::poison(&info)`, for example
    /// after a protocol error. Method-call syntax cannot mark a connection by accident.
    pub fn poison(this: &Self) {
        this.inner.poisoned.store(true, Ordering::Relaxed);
    }

    /// Returns the maximum connection age before recycling.
    ///
    /// `None` means that no maximum age is set. The server or keep-alive probes can
    /// still close the connection.
    #[must_use]
    pub fn max_age(&self) -> Option<Duration> {
        self.inner.max_age
    }

    /// Returns `true` if [`age`](Self::age) is greater than [`max_age`](Self::max_age).
    ///
    /// This returns `false` when `max_age` is `None`. A connection is not expired when
    /// its age equals `max_age`.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.max_age().is_some_and(|max_age| self.age() > max_age)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::sync::atomic::AtomicU64;

    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// Returns a clock at `start + <offset>` and a function that changes the offset.
    fn manual_clock() -> (impl Fn() -> Instant + Send + Sync + 'static, impl Fn(Duration)) {
        let base = Instant::now();
        let offset = Arc::new(AtomicU64::new(0));
        let setter = Arc::clone(&offset);

        let clock = move || base + Duration::from_nanos(offset.load(Ordering::Relaxed));
        let set = move |d: Duration| setter.store(u64::try_from(d.as_nanos()).unwrap(), Ordering::Relaxed);

        (clock, set)
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn assert_connection_info_type() {
        static_assertions::assert_impl_all!(ConnectionInfo: Send, Sync, Clone, Debug);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn age_tracks_the_clock_relative_to_start() {
        let (clock, set) = manual_clock();
        set(secs(5)); // Create the connection at 5 seconds.
        let info = ConnectionInfo::new(clock, PoolIndex::new(0), None);

        assert_eq!(info.age(), Duration::ZERO); // No time elapsed.
        set(secs(8));
        assert_eq!(info.age(), secs(3)); // Each call measures from the start time.
        set(Duration::ZERO);
        assert_eq!(info.age(), Duration::ZERO); // A clock rewind saturates at zero.
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn exposes_metadata_and_poison_flag() {
        let info = ConnectionInfo::new(Instant::now, PoolIndex::new(7), Some(secs(60)));

        assert_eq!(info.pool_index(), PoolIndex::new(7));
        assert_eq!(info.max_age(), Some(secs(60)));
        assert!(!info.is_poisoned());

        ConnectionInfo::poison(&info);
        ConnectionInfo::poison(&info); // A second call has no additional effect.
        assert!(info.is_poisoned());

        assert_eq!(ConnectionInfo::new(Instant::now, PoolIndex::new(0), None).max_age(), None);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn clones_share_state() {
        let (clock, set) = manual_clock();
        let info = ConnectionInfo::new(clock, PoolIndex::new(4), Some(secs(30)));
        let clone = info.clone();

        ConnectionInfo::poison(&info);
        set(secs(9));

        assert!(clone.is_poisoned());
        assert_eq!(clone.age(), info.age());
        assert_eq!(clone.pool_index(), PoolIndex::new(4));
        assert_eq!(clone.max_age(), Some(secs(30)));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn debug_lists_metadata_and_redacts_closure() {
        let debug = format!("{:?}", ConnectionInfo::new(Instant::now, PoolIndex::new(7), Some(secs(30))));

        assert!(debug.contains("NowFn(<closure>)"), "{debug}");
        assert!(debug.contains("pool_index"), "{debug}");
        assert!(debug.contains("max_age"), "{debug}");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn is_expired_false_without_max_age() {
        let info = ConnectionInfo::new(Instant::now, PoolIndex::new(0), None);
        assert!(!info.is_expired());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn is_expired_true_once_age_exceeds_max_age() {
        let (clock, set) = manual_clock();
        let info = ConnectionInfo::new(clock, PoolIndex::new(0), Some(secs(5)));
        assert!(!info.is_expired());
        set(secs(10));
        assert!(info.is_expired());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn is_expired_uses_strictly_greater_than_at_max_age_boundary() {
        // The connection stays valid when its age equals `max_age`.
        let (clock, set) = manual_clock();
        let info = ConnectionInfo::new(clock, PoolIndex::new(0), Some(secs(5)));
        set(secs(5));
        assert_eq!(info.age(), secs(5));
        assert!(!info.is_expired());
        set(secs(5) + Duration::from_nanos(1));
        assert!(info.is_expired());
    }
}
