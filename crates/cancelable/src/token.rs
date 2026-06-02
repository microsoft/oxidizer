// Copyright (c) Microsoft Corporation.

use std::any::type_name;
use std::fmt;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

type Subscriber = Box<dyn FnOnce() + Send>;

/// Shared state backing one or more [`CancellationToken`] handles.
struct Inner {
    /// Cancellation signal
    canceled: AtomicBool,

    /// Subscribers to notify on cancellation
    ///
    /// `Some(vec)` → not yet canceled; subscribers accumulate here.
    /// `None` → already canceled; new subscribers fire immediately.
    subscribers: Mutex<Option<Vec<Subscriber>>>,
}

impl Inner {
    fn new(canceled: bool) -> Self {
        Self {
            canceled: AtomicBool::new(canceled),
            subscribers: Mutex::new(if canceled { None } else { Some(Vec::new()) }),
        }
    }

    /// Returns `true` if cancellation has been requested.
    #[must_use]
    fn is_cancelled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    /// Attempt to change the `canceled` signal from `false` to `true`.
    ///
    /// Returns `true` on success, signaling that the caller is responsible for notifying subscribers.
    fn try_set_cancelled(&self) -> bool {
        self.canceled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Signal cancellation and notify subscribers
    fn cancel_and_notify(&self) {
        if self.try_set_cancelled() {
            let subscribers = match self.subscribers.lock() {
                // Lock, take contents, and unlock
                Ok(mut guard) => guard.take().unwrap_or_default(),

                // Lock has been poisoned, which means we can't read the subscriber list.
                Err(_) => Vec::default(),
            };

            // Notify from outside the lock
            for f in subscribers {
                f();
            }
        }
    }

    /// Subscribe to the cancellation notification
    ///
    /// If cancellation has already occurred, the callback fires immediately.
    fn subscribe(&self, callback: Subscriber) {
        if let Ok(mut guard) = self.subscribers.lock() {
            // Subscribers is `Some(...)` which means we haven't notified them yet
            // (e.g. not canceled, and not mid-cancellation). Add to the list.
            if let Some(list) = guard.as_mut() {
                list.push(callback);
                return;
            }

            // Subscribers list was `None`, meaning cancellation has already occurred
            // and all subscribers have already been notified.
            //
            // Fall through to release the lock, then notify immediately.
        } else {
            // Lock has been poisoned, which means we can't add to the subscriber list.
            //
            // The token source is unusable. Send the notification immediately.
        }

        callback();
    }
}

impl Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let subscribers = match self.subscribers.try_lock() {
            Ok(lock) => match &*lock {
                Some(s) => format!("Mutex(Some({} subscriber closure(s)))", s.len()),
                None => String::from("Mutex(None)"),
            },
            Err(_) => String::from("locked subscriber list"),
        };

        f.debug_struct(type_name::<Self>())
            .field("canceled", &self.canceled.load(Ordering::Acquire))
            .field("subscribers", &subscribers)
            .finish()
    }
}

/// A lightweight, cloneable handle for observing whether cancellation has been
/// requested.
///
/// Tokens are obtained from a [`CancellationTokenSource`] via
/// [`token()`](CancellationTokenSource::token) and can be passed throughout a
/// call graph so that any layer can cooperatively check for cancellation.
///
/// # Examples
///
/// ```
/// # fn example() {
/// use cancelable::CancellationTokenSource;
///
/// let source = CancellationTokenSource::new();
/// let token = source.token();
///
/// assert!(!token.is_cancelled());
/// source.cancel();
/// assert!(token.is_cancelled());
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<Inner>,
}

impl CancellationToken {
    /// Create a new cancellation token
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::new(false)),
        }
    }

    /// Returns a token that is already canceled.
    ///
    /// Useful for testing or for immediately signaling cancellation.
    #[must_use]
    pub fn cancelled() -> Self {
        Self {
            inner: Arc::new(Inner::new(true)),
        }
    }

    /// Returns `true` if cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// Get a weak reference to the shared state
    fn weak_ref(&self) -> Weak<Inner> {
        Arc::downgrade(&self.inner)
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Controls cancellation for one or more [`CancellationToken`]s
///
/// Create a source, distribute tokens via [`token()`](Self::token), and call
/// [`cancel()`](Self::cancel) when the operation should stop.
///
/// Dropping a `CancellationTokenSource` does **not** cancel its tokens.
/// Outstanding tokens simply remain in their current state.
#[derive(Debug, Default)]
pub struct CancellationTokenSource {
    token: CancellationToken,
}

impl CancellationTokenSource {
    /// Creates a new, independent cancellation source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// Creates a source linked to the parent tokens.
    ///
    /// The returned source's token reports [`is_cancelled()`] when:
    /// - [`cancel()`](Self::cancel) is called directly on this source, **or**
    /// - *any* parent token is canceled
    ///
    /// Linked sources work by registering a subscriber on each parent token.
    /// When any parent is canceled, we get a notification (callback), and use it to self-cancel.
    ///
    /// [`is_cancelled()`]: CancellationToken::is_cancelled
    #[must_use]
    pub fn linked(parents: &[CancellationToken]) -> Self {
        let source = Self::new();

        if !parents.is_empty() {
            let weak = source.token.weak_ref();

            for parent in parents {
                let weak = Weak::clone(&weak);
                parent.inner.subscribe(Box::new(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.cancel_and_notify();
                    }
                }));
            }
        }

        source
    }

    /// Returns a [`CancellationToken`] associated with this source.
    ///
    /// All tokens from the same source share the same cancellation state.
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Requests cancellation.
    ///
    /// All tokens obtained from this source will report
    /// [`is_cancelled() == true`](CancellationToken::is_cancelled) after this
    /// call, and all registered subscribers will be notified. Calling `cancel`
    /// more than once has no additional effect.
    ///
    /// Subscriber callbacks run synchronously on the calling thread. If any
    /// callback panics, the panic propagates immediately, and remaining
    /// callbacks will not run.
    pub fn cancel(&self) {
        self.token.inner.cancel_and_notify();
    }

    /// Returns `true` if cancellation has been requested on this source
    /// or on any parent token (for linked sources).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Registers a callback to invoke when cancellation occurs.
    ///
    /// If this source is already canceled, the callback fires immediately
    /// on the calling thread.
    pub fn subscribe(&self, callback: impl FnOnce() + Send + 'static) {
        self.token.inner.subscribe(Box::new(callback));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn new_source_is_not_cancelled() {
        let source = CancellationTokenSource::new();
        assert!(!source.is_cancelled());
        assert!(!source.token().is_cancelled());
    }

    #[test]
    fn cancel_propagates_to_token() {
        let source = CancellationTokenSource::new();
        let token = source.token();

        source.cancel();

        assert!(token.is_cancelled());
        assert!(source.is_cancelled());
    }

    #[test]
    fn cancel_is_idempotent() {
        let source = CancellationTokenSource::new();
        source.cancel();
        source.cancel();
        assert!(source.is_cancelled());
    }

    #[test]
    fn multiple_tokens_share_state() {
        let source = CancellationTokenSource::new();
        let t1 = source.token();
        let t2 = source.token();

        source.cancel();

        assert!(t1.is_cancelled());
        assert!(t2.is_cancelled());
    }

    #[test]
    fn cloned_token_shares_state() {
        let source = CancellationTokenSource::new();
        let t1 = source.token();
        let t2 = t1.clone();

        source.cancel();

        assert!(t1.is_cancelled());
        assert!(t2.is_cancelled());
    }

    #[test]
    fn cancelled_token_is_cancelled() {
        let token = CancellationToken::cancelled();
        assert!(token.is_cancelled());
    }

    #[test]
    fn default_token_is_not_cancelled() {
        let token = CancellationToken::default();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn default_source_is_not_cancelled() {
        let source = CancellationTokenSource::default();
        assert!(!source.is_cancelled());
    }

    #[test]
    fn linked_cancels_when_parent_cancels() {
        let parent = CancellationTokenSource::new();

        let linked = CancellationTokenSource::linked(&[parent.token()]);
        let linked_token = linked.token();

        assert!(!linked_token.is_cancelled());
        parent.cancel();
        assert!(linked_token.is_cancelled());
    }

    #[test]
    fn linked_cancels_when_any_parent_cancels() {
        let p1 = CancellationTokenSource::new();
        let p2 = CancellationTokenSource::new();
        let linked = CancellationTokenSource::linked(&[p1.token(), p2.token()]);

        assert!(!linked.is_cancelled());
        p2.cancel();
        assert!(linked.is_cancelled());
    }

    #[test]
    fn linked_cancels_directly() {
        let parent = CancellationTokenSource::new();
        let linked = CancellationTokenSource::linked(&[parent.token()]);

        linked.cancel();

        assert!(linked.is_cancelled());
        assert!(!parent.is_cancelled(), "cancelling child must not propagate to parent");
    }

    #[test]
    fn linked_chain_propagates() {
        let root = CancellationTokenSource::new();
        let mid = CancellationTokenSource::linked(&[root.token()]);
        let leaf = CancellationTokenSource::linked(&[mid.token()]);

        assert!(!leaf.is_cancelled());
        root.cancel();
        assert!(leaf.is_cancelled());
    }

    #[test]
    fn linked_from_already_cancelled_parent() {
        let parent = CancellationTokenSource::new();
        parent.cancel();

        let linked = CancellationTokenSource::linked(&[parent.token()]);
        assert!(linked.is_cancelled());
    }

    #[test]
    fn dropping_source_does_not_cancel_token() {
        let token = {
            let source = CancellationTokenSource::new();
            source.token()
        };
        assert!(!token.is_cancelled());
    }

    #[test]
    fn dropped_linked_source_does_not_notify_on_parent_cancel() {
        let parent = CancellationTokenSource::new();
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let linked = CancellationTokenSource::linked(&[parent.token()]);
            let c = Arc::clone(&counter);
            linked.subscribe(move || {
                c.fetch_add(1, Ordering::Relaxed);
            });

            // linked dropped here
        }

        parent.cancel();
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancel_visible_across_threads() {
        let source = CancellationTokenSource::new();
        let token = source.token();

        let handle = std::thread::spawn(move || {
            while !token.is_cancelled() {
                std::hint::spin_loop();
            }
            true
        });

        source.cancel();
        assert!(handle.join().unwrap());
    }

    #[test]
    fn linked_cancellation_is_visible_across_threads() {
        let parent = CancellationTokenSource::new();
        let linked = CancellationTokenSource::linked(&[parent.token()]);
        let linked_token = linked.token();

        let handle = std::thread::spawn(move || {
            while !linked_token.is_cancelled() {
                std::hint::spin_loop();
            }
            true
        });

        parent.cancel();
        assert!(handle.join().unwrap());
    }

    #[test]
    fn subscribers_are_notified_on_cancel() {
        let source = CancellationTokenSource::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&counter);
        source.subscribe(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });

        source.cancel();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn subscribers_are_notified_immediately_if_already_cancelled() {
        let source = CancellationTokenSource::new();
        source.cancel();

        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        source.subscribe(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });

        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn all_subscribers_are_notified() {
        let source = CancellationTokenSource::new();
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..5 {
            let c = Arc::clone(&counter);
            source.subscribe(move || {
                c.fetch_add(1, Ordering::Relaxed);
            });
        }

        source.cancel();
        assert_eq!(counter.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn subscribers_are_only_notified_once_on_double_cancel() {
        let source = CancellationTokenSource::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&counter);
        source.subscribe(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });

        source.cancel();
        source.cancel();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn linked_subscriber_is_notified_on_parent_cancel() {
        let parent = CancellationTokenSource::new();
        let linked = CancellationTokenSource::linked(&[parent.token()]);
        let counter = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&counter);
        linked.subscribe(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });

        parent.cancel();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn chained_linked_subscribers_are_notified() {
        let root = CancellationTokenSource::new();
        let mid = CancellationTokenSource::linked(&[root.token()]);
        let leaf = CancellationTokenSource::linked(&[mid.token()]);

        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        leaf.subscribe(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });

        root.cancel();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
}
