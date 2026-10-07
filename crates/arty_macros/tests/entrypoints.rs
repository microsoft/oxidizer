// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises the public proc-macro shims without a dependency cycle back to Arty.

extern crate self as arty;

/// Minimal runtime fixture matching the paths emitted by the proc macros.
pub mod runtime {
    use std::cell::Cell;

    thread_local! {
        static STOP_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    /// Private hooks used by generated entry points.
    pub mod __private {
        /// Resumes a root-task or shutdown failure.
        pub fn resume_error(payload: Box<dyn std::any::Any + Send + 'static>) -> ! {
            std::panic::resume_unwind(payload)
        }
    }

    /// Capabilities injected into the fixture task.
    #[derive(Debug)]
    pub struct Builtins {
        /// Fixture payload.
        pub value: usize,
        /// Configured worker limit.
        pub workers: usize,
    }

    /// Fixture worker-count policy.
    #[derive(Debug, Clone, Copy)]
    pub struct WorkersPolicy(usize);

    impl WorkersPolicy {
        /// Caps the fixture worker count.
        #[must_use]
        pub const fn at_most(count: usize) -> Self {
            Self(count)
        }
    }

    /// Fixture runtime owner.
    #[derive(Debug)]
    pub struct Runtime {
        scheduler: RuntimeScheduler,
    }

    impl Runtime {
        /// Creates a fixture runtime with default settings.
        ///
        /// # Errors
        ///
        /// Returns an error if the fixture builder rejects its configuration.
        pub fn new() -> Result<Self, &'static str> {
            Self::builder().build()
        }

        /// Creates a fixture runtime builder.
        #[must_use]
        pub const fn builder() -> RuntimeBuilder {
            RuntimeBuilder { workers: 2 }
        }

        /// Borrows the fixture scheduler.
        #[must_use]
        pub const fn scheduler(&self) -> &RuntimeScheduler {
            &self.scheduler
        }

        /// Stops the fixture runtime.
        ///
        /// # Errors
        ///
        /// Returns an error for an invalid fixture runtime.
        pub fn stop(self) -> std::thread::Result<()> {
            if self.scheduler.workers == 0 {
                return Err(Box::new("worker count must be greater than zero"));
            }
            STOP_CALLS.set(STOP_CALLS.get().saturating_add(1));
            Ok(())
        }
    }

    /// Fixture runtime builder.
    #[derive(Debug)]
    pub struct RuntimeBuilder {
        workers: usize,
    }

    impl RuntimeBuilder {
        /// Sets the fixture worker policy.
        #[must_use]
        pub const fn workers(mut self, policy: WorkersPolicy) -> Self {
            self.workers = policy.0;
            self
        }

        /// Builds the fixture runtime.
        ///
        /// # Errors
        ///
        /// Returns an error for a zero worker count.
        pub fn build(self) -> Result<Runtime, &'static str> {
            if self.workers == 0 {
                return Err("worker count must be greater than zero");
            }
            Ok(Runtime {
                scheduler: RuntimeScheduler { workers: self.workers },
            })
        }
    }

    /// Fixture runtime-wide scheduler.
    #[derive(Debug)]
    pub struct RuntimeScheduler {
        workers: usize,
    }

    impl RuntimeScheduler {
        /// Runs the generated root task.
        ///
        /// # Errors
        ///
        /// Returns the task's panic payload.
        pub fn block_on<F, Fut, R>(&self, factory: F) -> std::thread::Result<R>
        where
            F: FnOnce(Builtins) -> Fut,
            Fut: Future<Output = R>,
        {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                futures::executor::block_on(factory(Builtins {
                    value: 41,
                    workers: self.workers,
                }))
            }))
        }
    }

    /// Reports how many fixture runtimes have stopped.
    #[must_use]
    pub fn stop_calls() -> usize {
        STOP_CALLS.get()
    }
}

#[arty_macros::main]
async fn entrypoint(cx: runtime::Builtins) -> usize {
    cx.value.saturating_add(1)
}

#[arty_macros::test(workers = 1)]
async fn runtime_test(cx: runtime::Builtins) {
    assert_eq!(cx.value, 41);
    assert_eq!(cx.workers, 1);
}

#[test]
fn main_macro_executes_and_stops_the_runtime() {
    runtime_test();
    assert_eq!(entrypoint(), 42);
    assert_eq!(runtime::stop_calls(), 2);
}
