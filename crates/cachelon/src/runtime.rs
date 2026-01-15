// Copyright (c) Microsoft Corporation.

//! Runtime abstraction for background tasks.
//!
//! This module provides runtime abstractions for spawning background refresh tasks
//! that work with both tokio and oxidizer runtimes.
//! TODO: Change this to use arty when available

#![allow(dead_code, reason = "runtime module is conditionally used")]

use tick::Clock;

#[derive(Debug, Clone)]
pub(crate) struct Runtime {
    kind: RuntimeKind,
}

#[derive(Debug, Clone)]
#[cfg(any(feature = "tokio", test))]
pub(crate) enum RuntimeKind {
    #[cfg(any(feature = "tokio", test))]
    Tokio(TokioDeps),

    #[cfg(not(any(feature = "tokio", test)))]
    NoFeatures,
}

#[cfg(any(feature = "tokio", test))]
#[derive(Debug, Clone)]
#[fundle::deps]
pub struct TokioDeps {
    pub clock: Clock,
}

impl Runtime {
    #[cfg(any(feature = "tokio", test))]
    #[must_use]
    pub(crate) fn new_tokio(deps: TokioDeps) -> Self {
        Self {
            kind: RuntimeKind::Tokio(deps),
        }
    }

    pub(crate) fn clock(&self) -> &Clock {
        match &self.kind {
            #[cfg(any(feature = "tokio", test))]
            RuntimeKind::Tokio(deps) => &deps.clock,
            #[cfg(not(any(feature = "tokio", test)))]
            RuntimeKind::NoFeatures => panic!("No runtime features enabled"),
        }
    }

    pub(crate) fn spawn<T>(&self, work: T)
    where
        T: std::future::Future<Output = ()> + Send + 'static,
    {
        match &self.kind {
            #[cfg(any(feature = "tokio", test))]
            RuntimeKind::Tokio { .. } => {
                tokio::spawn(work);
            }
            #[cfg(not(any(feature = "tokio", test)))]
            RuntimeKind::NoFeatures => {
                drop(work);
                panic!("No runtime features enabled");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokio_deps_new() {
        let clock = Clock::new_frozen();
        let deps = TokioDeps { clock };
        assert!(!format!("{:?}", deps).is_empty());
    }

    #[test]
    fn runtime_new_tokio() {
        let clock = Clock::new_frozen();
        let deps = TokioDeps { clock: clock.clone() };
        let runtime = Runtime::new_tokio(deps);
        assert!(!format!("{:?}", runtime).is_empty());
    }

    #[test]
    fn runtime_clock_tokio() {
        let clock = Clock::new_frozen();
        let deps = TokioDeps { clock: clock.clone() };
        let runtime = Runtime::new_tokio(deps);
        let runtime_clock = runtime.clock();
        // Just verify we can access the clock
        let _ = runtime_clock.instant();
    }

    #[test]
    fn runtime_clone() {
        let clock = Clock::new_frozen();
        let deps = TokioDeps { clock: clock.clone() };
        let runtime = Runtime::new_tokio(deps);
        let cloned = runtime.clone();
        assert!(!format!("{:?}", cloned).is_empty());
    }

    #[test]
    fn runtime_kind_debug() {
        let clock = Clock::new_frozen();
        let deps = TokioDeps { clock: clock.clone() };
        let runtime = Runtime::new_tokio(deps);
        let debug_str = format!("{:?}", runtime);
        assert!(debug_str.contains("Runtime"));
    }
}
