// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, OnceLock};
use std::task::Waker;

use arty_executor::{ShutdownCause, ShutdownError};

/// Shared runtime shutdown state.
#[derive(Clone, Default, derive_more::Debug)]
pub(crate) struct Shutdown {
    #[debug(ignore)]
    cause: Arc<OnceLock<ShutdownCause>>,
    #[debug(ignore)]
    panic_notifier: Arc<OnceLock<Waker>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShutdownState {
    Running,
    Started,
}

impl Shutdown {
    /// Starts shutdown and returns whether this call started it.
    ///
    /// The first published cause is authoritative. Later requests do not
    /// replace it, including a default cause without an error.
    pub(crate) fn begin(&self, cause: ShutdownCause) -> bool {
        self.cause.set(cause).is_ok()
    }

    pub(crate) fn initialize_panic_notifier(&self, notifier: Waker) {
        _ = self.panic_notifier.set(notifier);
    }

    pub(crate) fn panic_notifier(&self) -> Waker {
        self.panic_notifier.get().cloned().unwrap_or_else(|| Waker::noop().clone())
    }

    #[must_use]
    pub(crate) fn state(&self) -> ShutdownState {
        if self.cause.get().is_some() {
            ShutdownState::Started
        } else {
            ShutdownState::Running
        }
    }

    #[must_use]
    pub(crate) fn cause(&self) -> Option<ShutdownCause> {
        self.cause.get().cloned()
    }

    #[must_use]
    pub(crate) fn error(&self) -> Option<ShutdownError> {
        self.cause().and_then(ShutdownCause::into_error)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::{self, Debug, Display};
    use std::sync::Barrier;
    use std::thread;

    use static_assertions::assert_impl_all;

    use super::*;

    #[derive(Debug)]
    struct TestError(&'static str);

    impl Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl Error for TestError {}

    fn cause(message: &'static str) -> ShutdownCause {
        ShutdownError::new(TestError(message)).into()
    }

    #[test]
    fn default_cause_remains_authoritative() {
        assert_impl_all!(Shutdown: Clone, Debug, Default, Send, Sync);

        let shutdown = Shutdown::default();

        assert!(shutdown.begin(ShutdownCause::default()));
        assert!(!shutdown.begin(cause("later")));

        assert_eq!(shutdown.state(), ShutdownState::Started);
        assert!(shutdown.error().is_none());
    }

    #[test]
    fn error_cause_remains_authoritative() {
        let shutdown = Shutdown::default();

        assert!(shutdown.begin(cause("first")));
        assert!(!shutdown.begin(cause("later")));

        assert_eq!(shutdown.error().unwrap().inner().to_string(), "first");
    }

    #[test]
    fn concurrent_requests_publish_one_stable_cause() {
        let shutdown = Shutdown::default();
        let barrier = Arc::new(Barrier::new(3));
        let workers: [_; 2] = ["first", "second"].map(|message| {
            let shutdown = shutdown.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                shutdown.begin(cause(message))
            })
        });

        barrier.wait();
        let started = workers.map(|worker| worker.join().unwrap());

        assert_eq!(started.into_iter().filter(|started| *started).count(), 1);
        let observed = shutdown.error().unwrap().inner().to_string();
        assert!(matches!(observed.as_str(), "first" | "second"));
        assert_eq!(shutdown.error().unwrap().inner().to_string(), observed);
    }
}
