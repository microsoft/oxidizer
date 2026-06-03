// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

enum Subscriber {
    /// An external callback for arbitrary subscriber logic.
    External(Box<dyn FnOnce() + Send>),
    /// A weak reference to a linked child's shared state, avoiding a heap
    /// allocation for the common parent/child propagation path.
    Linked(Weak<Inner>),
}

impl Subscriber {
    fn notify(self) {
        match self {
            Self::External(f) => f(),
            Self::Linked(weak) => {
                if let Some(inner) = weak.upgrade() {
                    inner.cancel_and_notify();
                }
            }
        }
    }

    fn matches_linked(&self, target: &Weak<Inner>) -> bool {
        match self {
            Self::External(_) => false,
            Self::Linked(inner) => inner.ptr_eq(target),
        }
    }
}

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
        // Use acquire/release ordering to ensure cancelable reads occur before
        // draining the subscriber list. May not be strictly necessary since
        // the subscriber list is protected by a lock instead of atomic.
        self.canceled.load(Ordering::Acquire)
    }

    /// Attempt to change the `canceled` signal from `false` to `true`.
    ///
    /// Returns `true` on success, signaling that the caller is responsible for notifying subscribers.
    fn try_set_cancelled(&self) -> bool {
        // Use acquire/release ordering to ensure cancelable reads/updates occur
        // before draining the subscriber list. May not be strictly necessary since
        // the subscriber list is protected by a lock instead of atomic.
        self.canceled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Signal cancellation and notify subscribers
    fn cancel_and_notify(&self) {
        if !self.try_set_cancelled() {
            // Already canceled by someone else. They will notify.
            return;
        }

        // Lock, take subscribers, and unlock
        let subscribers = self
            .subscribers
            .lock()
            .expect("subscriber lock should not be poisoned because lock is never held over fallible or unsafe calls")
            .take()
            .unwrap_or_default();

        // Notify from outside the lock
        for subscriber in subscribers {
            subscriber.notify();
        }
    }

    /// Subscribe to the cancellation notification
    ///
    /// If cancellation has already occurred, the callback fires immediately.
    fn subscribe(&self, callback: Subscriber) {
        let mut guard = self
            .subscribers
            .lock()
            .expect("subscriber lock should not be poisoned because lock is never held over fallible or unsafe calls");

        // Subscribers is `Some(...)` which means we haven't notified them yet
        // (e.g. not canceled, and not mid-cancellation). Add to the list.
        if let Some(list) = guard.as_mut() {
            list.push(callback);
            return;
        }

        // Subscribers list was `None`, meaning cancellation has already occurred
        // and all subscribers have already been notified.
        //
        // Release the lock, then notify immediately.
        drop(guard);
        callback.notify();
    }

    /// Remove the linked child token from the list of subscribers.
    ///
    /// This is a no-op if cancellation has already occurred (the list is `None`).
    ///
    /// # Panics
    ///
    /// Panics when the lock protecting the subscriber list is poisoned. This
    /// happens when another thread, which had been holding the lock, panicked.
    #[cfg_attr(test, mutants::skip)] // Mutation breaks list iteration, causing tests to run forever.
    fn unsubscribe_linked_child(&self, child: &Weak<Self>) {
        let mut guard = self.subscribers.lock().expect("subscriber lock is poisoned");

        if let Some(list) = guard.as_mut() {
            let mut i = 0;
            while i < list.len() {
                if list[i].matches_linked(child) {
                    list.swap_remove(i);
                } else {
                    i += 1;
                }
            }
        }
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

/// A lightweight, cloneable handle for observing a cancellation signal.
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

/// Controls cancellation, providing a shared [`CancellationToken`].
///
/// Create a source, distribute tokens via [`token()`](Self::token), and call
/// [`cancel()`](Self::cancel) to signal when the operation should stop.
///
/// # Drop Behavior
///
/// Dropping a [`CancellationTokenSource`] does **not** cancel its tokens.
/// Outstanding tokens simply remain in their current state.
///
/// # Linked Parents
///
/// When a source is [`linked()`](CancellationTokenSource::linked) to a set of
/// parents, it registers to receive notifications from each parent. When the
/// source is later dropped, it unregisters from each of the parents. This
/// ensures that long-lived parents only track and notify active children.
#[derive(Debug, Default)]
pub struct CancellationTokenSource {
    token: CancellationToken,
    /// Parents to which this source is linked
    parent_refs: Vec<Weak<Inner>>,
}

impl CancellationTokenSource {
    /// Creates a new, independent cancellation source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            parent_refs: Vec::new(),
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
    /// On drop, this source unregisters itself from each parent.
    ///
    /// [`is_cancelled()`]: CancellationToken::is_cancelled
    #[must_use]
    pub fn linked(parents: &[CancellationToken]) -> Self {
        let source = Self {
            token: CancellationToken::new(),
            parent_refs: parents.iter().map(CancellationToken::weak_ref).collect(),
        };

        if !parents.is_empty() {
            let weak = source.token.weak_ref();

            for parent in parents {
                parent.inner.subscribe(Subscriber::Linked(Weak::clone(&weak)));
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
    ///
    /// This callback cannot be unregistered.
    pub fn subscribe(&self, callback: impl FnOnce() + Send + 'static) {
        self.token.inner.subscribe(Subscriber::External(Box::new(callback)));
    }
}

impl Drop for CancellationTokenSource {
    fn drop(&mut self) {
        if self.parent_refs.is_empty() {
            return;
        }

        let weak = self.token.weak_ref();
        for parent_ref in &self.parent_refs {
            if let Some(inner) = parent_ref.upgrade() {
                inner.unsubscribe_linked_child(&weak);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::thread::JoinHandle;

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

        {
            let linked = CancellationTokenSource::linked(&[parent.token()]);
            linked.subscribe(|| panic!("should not be called"));

            // linked dropped here
        }

        parent.cancel();
    }

    fn start_cancellation_polling_thread(token: CancellationToken) -> JoinHandle<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let counter = Arc::new(AtomicUsize::new(0));

        let thread_counter = Arc::clone(&counter);
        let thread_handle = std::thread::spawn(move || {
            while !token.is_cancelled() {
                thread_counter.fetch_add(1, Ordering::Relaxed);
                assert!(std::time::Instant::now() < deadline, "thread did not finish in time");
                std::hint::spin_loop();
            }
        });

        // wait for the thread to start running
        while counter.load(Ordering::Relaxed) == 0 {
            assert!(std::time::Instant::now() < deadline, "thread did not start in time");
            std::hint::spin_loop();
        }

        thread_handle
    }

    #[test]
    fn cancel_visible_across_threads() {
        let source = CancellationTokenSource::new();
        let handle = start_cancellation_polling_thread(source.token());
        source.cancel();
        handle.join().expect("thread should complete successfully");
    }

    #[test]
    fn linked_cancellation_is_visible_across_threads() {
        let parent = CancellationTokenSource::new();
        let linked = CancellationTokenSource::linked(&[parent.token()]);
        let handle = start_cancellation_polling_thread(linked.token());
        parent.cancel();
        handle.join().expect("thread should complete successfully");
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

    /// Returns the number of subscribers currently registered, or `None` if
    /// cancellation has already drained the list.
    fn subscriber_count(inner: &Inner) -> Option<usize> {
        inner
            .subscribers
            .lock()
            .expect("subscriber lock is poisoned")
            .as_ref()
            .map(Vec::len)
    }

    #[test]
    fn unsubscribe_linked_child_removes_matching_entry() {
        let parent = Inner::new(false);
        let child = Arc::new(Inner::new(false));
        let weak = Arc::downgrade(&child);

        parent.subscribe(Subscriber::Linked(Weak::clone(&weak)));
        assert_eq!(subscriber_count(&parent), Some(1));

        parent.unsubscribe_linked_child(&weak);
        assert_eq!(subscriber_count(&parent), Some(0));
    }

    #[test]
    fn unsubscribe_linked_child_removes_all_matching_entries() {
        let parent = Inner::new(false);
        let child = Arc::new(Inner::new(false));
        let weak = Arc::downgrade(&child);

        parent.subscribe(Subscriber::Linked(Weak::clone(&weak)));
        parent.subscribe(Subscriber::Linked(Weak::clone(&weak)));
        assert_eq!(subscriber_count(&parent), Some(2));

        parent.unsubscribe_linked_child(&weak);
        assert_eq!(subscriber_count(&parent), Some(0));
    }

    #[test]
    fn unsubscribe_linked_child_leaves_other_linked_subscribers() {
        let parent = Inner::new(false);
        let child = Arc::new(Inner::new(false));
        let child_other = Arc::new(Inner::new(false));
        let weak = Arc::downgrade(&child);
        let weak_other = Arc::downgrade(&child_other);

        parent.subscribe(Subscriber::Linked(Weak::clone(&weak)));
        parent.subscribe(Subscriber::Linked(Weak::clone(&weak_other)));
        assert_eq!(subscriber_count(&parent), Some(2));

        parent.unsubscribe_linked_child(&weak);
        assert_eq!(subscriber_count(&parent), Some(1));

        // Cancelling the parent should still propagate to child_other
        parent.cancel_and_notify();
        assert!(child_other.is_cancelled());
        assert!(!child.is_cancelled());
    }

    #[test]
    fn unsubscribe_linked_child_leaves_external_subscribers() {
        let parent = Inner::new(false);
        let child = Arc::new(Inner::new(false));
        let weak = Arc::downgrade(&child);
        let counter = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&counter);
        parent.subscribe(Subscriber::External(Box::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
        })));
        parent.subscribe(Subscriber::Linked(Weak::clone(&weak)));
        assert_eq!(subscriber_count(&parent), Some(2));

        parent.unsubscribe_linked_child(&weak);
        assert_eq!(subscriber_count(&parent), Some(1));

        parent.cancel_and_notify();
        assert_eq!(counter.load(Ordering::Relaxed), 1, "external subscriber should still fire");
        assert!(!child.is_cancelled(), "unsubscribed child should not be cancelled");
    }

    #[test]
    fn unsubscribe_linked_child_is_noop_when_already_cancelled() {
        let parent = Inner::new(false);
        let child = Arc::new(Inner::new(false));
        let weak = Arc::downgrade(&child);

        parent.subscribe(Subscriber::Linked(Weak::clone(&weak)));
        parent.cancel_and_notify();
        assert_eq!(subscriber_count(&parent), None);

        // Should not panic or have any effect
        parent.unsubscribe_linked_child(&weak);
        assert_eq!(subscriber_count(&parent), None);
    }

    #[test]
    fn unsubscribe_linked_child_is_noop_when_no_match() {
        let parent = Inner::new(false);
        let child_a = Arc::new(Inner::new(false));
        let child_b = Arc::new(Inner::new(false));
        let weak_a = Arc::downgrade(&child_a);
        let weak_b = Arc::downgrade(&child_b);

        parent.subscribe(Subscriber::Linked(Weak::clone(&weak_a)));
        assert_eq!(subscriber_count(&parent), Some(1));

        parent.unsubscribe_linked_child(&weak_b);
        assert_eq!(subscriber_count(&parent), Some(1));
    }

    #[test]
    fn drop_linked_source_unregisters_from_parent() {
        let parent = CancellationTokenSource::new();

        {
            let _linked = CancellationTokenSource::linked(&[parent.token()]);
            assert_eq!(subscriber_count(&parent.token.inner), Some(1));
            // _linked dropped here
        }

        assert_eq!(subscriber_count(&parent.token.inner), Some(0));
    }

    #[test]
    fn drop_linked_source_unregisters_from_all_parents() {
        let p1 = CancellationTokenSource::new();
        let p2 = CancellationTokenSource::new();

        {
            let _linked = CancellationTokenSource::linked(&[p1.token(), p2.token()]);
            assert_eq!(subscriber_count(&p1.token.inner), Some(1));
            assert_eq!(subscriber_count(&p2.token.inner), Some(1));
        }

        assert_eq!(subscriber_count(&p1.token.inner), Some(0));
        assert_eq!(subscriber_count(&p2.token.inner), Some(0));
    }

    #[test]
    fn drop_linked_source_leaves_sibling_subscriptions() {
        let parent = CancellationTokenSource::new();
        let sibling = CancellationTokenSource::linked(&[parent.token()]);

        {
            let _linked = CancellationTokenSource::linked(&[parent.token()]);
            assert_eq!(subscriber_count(&parent.token.inner), Some(2));
        }

        assert_eq!(subscriber_count(&parent.token.inner), Some(1));

        // Sibling should still receive cancellation
        parent.cancel();
        assert!(sibling.is_cancelled());
    }

    #[test]
    fn drop_linked_source_leaves_external_subscriptions() {
        let parent = CancellationTokenSource::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let c = Arc::clone(&counter);
        parent.subscribe(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });

        {
            let _linked = CancellationTokenSource::linked(&[parent.token()]);
            assert_eq!(subscriber_count(&parent.token.inner), Some(2));
        }

        assert_eq!(subscriber_count(&parent.token.inner), Some(1));

        parent.cancel();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn drop_independent_source_does_not_panic() {
        let _source = CancellationTokenSource::new();
        // No parents — drop should be a no-op without panicking
    }

    #[test]
    fn drop_linked_source_after_parent_cancelled_does_not_panic() {
        let parent = CancellationTokenSource::new();
        let linked = CancellationTokenSource::linked(&[parent.token()]);

        parent.cancel();
        assert!(linked.is_cancelled());

        // Subscriber list is already None; drop should not panic
        drop(linked);
    }

    #[test]
    fn linked_with_no_parents_behaves_like_independent_source() {
        let source = CancellationTokenSource::linked(&[]);
        assert!(!source.is_cancelled());

        source.cancel();
        assert!(source.is_cancelled());
    }

    #[test]
    fn debug_inner_not_cancelled_no_subscribers() {
        let source = CancellationTokenSource::new();
        let debug = format!("{source:?}");
        assert!(debug.contains("canceled: false"), "expected canceled: false, got: {debug}");
        assert!(debug.contains("0 subscriber closure(s)"), "expected 0 subscribers, got: {debug}");
    }

    #[test]
    fn debug_inner_not_cancelled_with_subscribers() {
        let source = CancellationTokenSource::new();
        source.subscribe(|| {});
        source.subscribe(|| {});
        let debug = format!("{source:?}");
        assert!(debug.contains("canceled: false"), "expected canceled: false, got: {debug}");
        assert!(debug.contains("2 subscriber closure(s)"), "expected 2 subscribers, got: {debug}");
    }

    #[test]
    fn debug_inner_cancelled() {
        let source = CancellationTokenSource::new();
        source.cancel();
        let debug = format!("{source:?}");
        assert!(debug.contains("canceled: true"), "expected canceled: true, got: {debug}");
        assert!(debug.contains("Mutex(None)"), "expected Mutex(None), got: {debug}");
    }

    #[test]
    fn debug_inner_while_mutex_locked() {
        let source = CancellationTokenSource::new();
        // Hold the subscriber lock from the current thread so try_lock fails
        // during Debug formatting.
        let _guard = source.token.inner.subscribers.lock().unwrap();
        let debug = format!("{source:?}");
        assert!(
            debug.contains("locked subscriber list"),
            "expected locked subscriber list, got: {debug}"
        );
    }
}
