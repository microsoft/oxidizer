// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `observed` event definitions for runtime telemetry.
//!
//! See the [runtime telemetry overview](crate::runtime#telemetry). Dimensions and log
//! attributes are data-classified as `SystemMetadata` via newtype wrappers. A
//! field that is itself a numeric metric *value* stays a plain number tagged
//! `#[unredacted]`, since redaction renders values as strings and the metric
//! pipeline only accepts numbers.

use std::thread::ThreadId as StdThreadId;

use data_privacy::classified;
use observed::event;

use super::{PANIC_MESSAGE, SYSTEM_METADATA};

/// A system-level metric count (processors, threads, bytes) recorded in telemetry.
#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
pub(crate) struct SystemMetricCount(pub i64);

impl From<usize> for SystemMetricCount {
    /// Converts a `usize` quantity, saturating at `i64::MAX` (counts this large never occur).
    fn from(value: usize) -> Self {
        Self(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

/// Dense index of a worker in the runtime's selected processor set.
///
/// This is a runtime-local ordinal, not a hardware processor ID.
#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
pub(crate) struct ProcessorIndex(pub i64);

impl From<usize> for ProcessorIndex {
    /// Converts a `usize` index, saturating at `i64::MAX` (indexes this large never occur).
    fn from(value: usize) -> Self {
        Self(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

/// Blocking worker pool mode label (`isolated` / `shared`).
#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
pub(crate) struct BlockingWorkerPoolMode(pub &'static str);

/// Scheduling route (`any`, `same_thread`, or `local`).
#[classified(SYSTEM_METADATA)]
#[derive(Clone, Copy)]
pub(crate) struct PlacementLabel(pub &'static str);

/// The name of an OS thread the runtime owns.
#[classified(SYSTEM_METADATA)]
#[derive(Clone)]
pub(crate) struct ThreadName(pub String);

impl ThreadName {
    /// Builds a thread name from a borrowed name.
    pub(crate) fn new(name: &str) -> Self {
        Self(name.to_owned())
    }
}

/// The identity of an OS thread, rendered from [`std::thread::ThreadId`].
#[classified(SYSTEM_METADATA)]
#[derive(Clone)]
pub(crate) struct ThreadId(pub String);

impl From<StdThreadId> for ThreadId {
    /// Renders the thread id via its `Debug` representation (the only stable view).
    fn from(id: StdThreadId) -> Self {
        Self(format!("{id:?}"))
    }
}

/// The message extracted from a caught panic payload.
#[classified(PANIC_MESSAGE)]
#[derive(Clone)]
pub(crate) struct PanicMessage(pub String);

/// A captured backtrace rendered as text.
#[cfg(any(debug_assertions, test))] // Only captured by the debug-only `Builtins` thread check.
#[classified(SYSTEM_METADATA)]
#[derive(Clone)]
pub(crate) struct BacktraceText(pub String);

/// Emitted once when a runtime has started and all async workers are live.
#[event("arty.rt.started")]
#[info("runtime started")]
#[counter(name = "arty.rt.started")]
pub(crate) struct RuntimeStarted {
    /// Processors available on the system.
    #[dimension(log = "processors.available", metric = "processors.available")]
    pub processors_available: SystemMetricCount,
    /// Processors actually used (one async worker each).
    #[dimension(log = "processors.used", metric = "processors.used")]
    pub processors_used: SystemMetricCount,
    #[dimension(log = "blocking_worker_pool.mode", metric = "blocking_worker_pool.mode")]
    pub blocking_worker_pool_mode: BlockingWorkerPoolMode,
    pub stack_size_bytes: SystemMetricCount,
}

/// Emitted once when a runtime fails to start.
#[event("arty.rt.start_failed")]
#[error("runtime failed to start")]
#[counter(name = "arty.rt.start_failed")]
pub(crate) struct RuntimeStartFailed {
    #[dimension(log = "blocking_worker_pool.mode", metric = "blocking_worker_pool.mode")]
    pub blocking_worker_pool_mode: BlockingWorkerPoolMode,
}

/// Emitted when runtime shutdown begins.
#[event("arty.rt.stopping")]
#[debug("runtime stopping")]
pub(crate) struct RuntimeStopping;

/// Emitted when the runtime has fully stopped (pairs with `stopping`).
#[event("arty.rt.stopped")]
#[info("runtime stopped")]
pub(crate) struct RuntimeStopped;

/// Emitted when an async worker thread starts.
#[event("arty.rt.async_worker.started")]
#[debug("async worker started")]
pub(crate) struct AsyncWorkerStarted {
    #[dimension(log = "processor.index")]
    pub processor_index: ProcessorIndex,
}

/// Emitted when an async worker thread begins shutting down.
#[event("arty.rt.async_worker.stopped")]
#[debug("async worker stopped")]
pub(crate) struct AsyncWorkerStopped {
    #[dimension(log = "processor.index")]
    pub processor_index: ProcessorIndex,
}

/// Running total of live async workers (`+1` start, `-1` stop).
#[event("arty.rt.async_worker.active")]
#[updown_counter(delta, name = "arty.rt.async_worker.active")]
pub(crate) struct AsyncWorkerActive {
    #[unredacted]
    pub delta: i64,
}

/// Emitted when a task is spawned.
#[event("arty.rt.task.spawned")]
#[counter(name = "arty.rt.task.spawned")]
pub(crate) struct TaskSpawned {
    #[dimension(metric = "placement")]
    pub placement: PlacementLabel,
}

/// Emitted when a task completes successfully.
#[event("arty.rt.task.succeeded")]
#[counter(name = "arty.rt.task.succeeded")]
pub(crate) struct TaskSucceeded;

/// Emitted when a task panics.
#[event("arty.rt.task.panicked")]
#[error("task panicked")]
#[counter(name = "arty.rt.task.panicked")]
pub(crate) struct TaskPanicked;

/// Emitted on the owner thread just before a runtime-owned OS thread is spawned.
#[event("arty.rt.thread.spawn")]
#[debug("spawning thread")]
pub(crate) struct ThreadSpawn {
    #[dimension(log = "thread.name")]
    pub name: ThreadName,
    #[dimension(log = "thread.stack_size_bytes")]
    pub stack_size_bytes: Option<SystemMetricCount>,
    #[dimension(log = "thread.owner.name")]
    pub owner_name: Option<ThreadName>,
    #[dimension(log = "thread.owner.id")]
    pub owner_id: ThreadId,
}

/// Emitted on a runtime-owned OS thread once it begins running.
#[event("arty.rt.thread.started")]
#[debug("thread started")]
pub(crate) struct ThreadStarted {
    #[dimension(log = "thread.name")]
    pub name: Option<ThreadName>,
    #[dimension(log = "arty.thread.id")]
    pub id: ThreadId,
}

/// Emitted on a runtime-owned OS thread when its work panics, before the panic
/// is resumed.
#[event("arty.rt.thread.panic")]
#[error("thread panicked")]
pub(crate) struct ThreadPanicked {
    #[dimension(log = "thread.name")]
    pub name: Option<ThreadName>,
    #[dimension(log = "arty.thread.id")]
    pub id: ThreadId,
    #[dimension(log = "panic.message")]
    pub message: PanicMessage,
}

/// Emitted on a runtime-owned OS thread as it exits normally.
#[event("arty.rt.thread.exiting")]
#[debug("thread exiting")]
pub(crate) struct ThreadExiting {
    #[dimension(log = "thread.name")]
    pub name: Option<ThreadName>,
    #[dimension(log = "arty.thread.id")]
    pub id: ThreadId,
}

/// Emitted (debug builds only) when `Builtins` is accessed from a thread other
/// than the one it was created on.
#[cfg(any(debug_assertions, test))] // Emitted only by the debug-only `Builtins` thread check.
#[event("arty.rt.builtins.thread_mismatch")]
#[warning("Builtins accessed from a different thread than it was created on")]
pub(crate) struct BuiltinsThreadMismatch {
    #[dimension(log = "thread.name")]
    pub name: ThreadName,
    #[dimension(log = "arty.thread.id")]
    pub id: ThreadId,
    #[dimension(log = "backtrace")]
    pub backtrace: BacktraceText,
}

/// Emitted when the blocking worker pool is already at its maximum size
/// and cannot grow to absorb a fresh overload.
#[event("arty.rt.blocking_worker.pool_saturated")]
#[warning("blocking worker pool is saturated and cannot grow")]
#[counter(name = "arty.rt.blocking_worker.pool_saturated")]
pub(crate) struct BlockingWorkerPoolSaturated {
    #[dimension(log = "blocking_worker_pool.mode", metric = "blocking_worker_pool.mode")]
    pub blocking_worker_pool_mode: BlockingWorkerPoolMode,
    #[dimension(log = "blocking_worker_pool.max_threads", metric = "blocking_worker_pool.max_threads")]
    pub max_threads: SystemMetricCount,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use data_privacy::RedactionEngine;
    use observed::{Severity, emit};
    use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};

    use super::*;

    #[test]
    fn runtime_started_has_expected_metadata() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            RuntimeStarted {
                processors_available: SystemMetricCount(8),
                processors_used: SystemMetricCount(2),
                blocking_worker_pool_mode: BlockingWorkerPoolMode("isolated"),
                stack_size_bytes: SystemMetricCount(1024),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.started", Severity::Info)
                .body("runtime started")
                .dimension("processors.available", "8")
                .dimension("processors.used", "2")
                .dimension("blocking_worker_pool.mode", "isolated")
                .dimension("stack_size_bytes", "1024")
                .metric()
        );
    }

    #[test]
    fn runtime_start_failed_has_expected_metadata() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            RuntimeStartFailed {
                blocking_worker_pool_mode: BlockingWorkerPoolMode("shared"),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.start_failed", Severity::Error)
                .body("runtime failed to start")
                .dimension("blocking_worker_pool.mode", "shared")
                .metric()
        );
    }

    #[test]
    fn runtime_stopping_and_stopped_pair_up() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(sink, RuntimeStopping);
        emit!(sink, RuntimeStopped);

        let events = processor.events();
        assert_eq!(
            events[0],
            ExpectedEvent::new("arty.rt.stopping", Severity::Debug).body("runtime stopping")
        );
        assert_eq!(
            events[1],
            ExpectedEvent::new("arty.rt.stopped", Severity::Info).body("runtime stopped")
        );
    }

    #[test]
    fn async_worker_events_carry_processor_index() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            AsyncWorkerStarted {
                processor_index: ProcessorIndex(3)
            }
        );
        emit!(
            sink,
            AsyncWorkerStopped {
                processor_index: ProcessorIndex(3)
            }
        );

        let events = processor.events();
        assert_eq!(
            events[0],
            ExpectedEvent::new("arty.rt.async_worker.started", Severity::Debug)
                .body("async worker started")
                .dimension("processor.index", "3")
        );
        assert_eq!(
            events[1],
            ExpectedEvent::new("arty.rt.async_worker.stopped", Severity::Debug)
                .body("async worker stopped")
                .dimension("processor.index", "3")
        );
    }

    #[test]
    fn async_worker_active_is_metric_only() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(sink, AsyncWorkerActive { delta: 1 });

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::without_severity("arty.rt.async_worker.active")
                .dimension("delta", 1i64)
                .metric()
        );
    }

    #[test]
    fn task_spawned_carries_placement() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            TaskSpawned {
                placement: PlacementLabel("any")
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::without_severity("arty.rt.task.spawned")
                .dimension("placement", "any")
                .metric()
        );
    }

    #[test]
    fn task_succeeded_and_panicked_are_distinct_events() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(sink, TaskSucceeded);
        emit!(sink, TaskPanicked);

        let events = processor.events();
        assert_eq!(events[0], ExpectedEvent::without_severity("arty.rt.task.succeeded").metric());
        assert_eq!(
            events[1],
            ExpectedEvent::new("arty.rt.task.panicked", Severity::Error)
                .body("task panicked")
                .metric()
        );
    }

    #[test]
    fn thread_spawn_carries_thread_and_owner_metadata() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            ThreadSpawn {
                name: ThreadName::new("oxidizer-async-1"),
                stack_size_bytes: Some(SystemMetricCount(2_097_152)),
                owner_name: Some(ThreadName::new("main")),
                owner_id: ThreadId("ThreadId(1)".to_owned()),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.thread.spawn", Severity::Debug)
                .body("spawning thread")
                .dimension("thread.name", "oxidizer-async-1")
                .dimension("thread.stack_size_bytes", "2097152")
                .dimension("thread.owner.name", "main")
                .dimension("thread.owner.id", "ThreadId(1)")
        );
    }

    #[test]
    fn thread_spawn_fills_absent_optional_fields() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            ThreadSpawn {
                name: ThreadName::new("oxidizer-async-1"),
                stack_size_bytes: None,
                owner_name: None,
                owner_id: ThreadId("ThreadId(1)".to_owned()),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.thread.spawn", Severity::Debug)
                .body("spawning thread")
                .dimension("thread.name", "oxidizer-async-1")
                .dimension("thread.stack_size_bytes", "n/a")
                .dimension("thread.owner.name", "n/a")
                .dimension("thread.owner.id", "ThreadId(1)")
        );
    }

    #[test]
    fn thread_started_carries_identity() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            ThreadStarted {
                name: Some(ThreadName::new("oxidizer-async-1")),
                id: ThreadId("ThreadId(7)".to_owned()),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.thread.started", Severity::Debug)
                .body("thread started")
                .dimension("thread.name", "oxidizer-async-1")
                .dimension("arty.thread.id", "ThreadId(7)")
        );
    }

    #[test]
    fn thread_panicked_carries_panic_message() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            ThreadPanicked {
                name: Some(ThreadName::new("oxidizer-async-1")),
                id: ThreadId("ThreadId(7)".to_owned()),
                message: PanicMessage("something broke".to_owned()),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.thread.panic", Severity::Error)
                .body("thread panicked")
                .dimension("thread.name", "oxidizer-async-1")
                .dimension("arty.thread.id", "ThreadId(7)")
                .dimension("panic.message", "something broke")
        );
    }

    #[test]
    fn panic_message_is_not_suppressed_with_system_metadata() {
        let engine = RedactionEngine::builder().suppress_redaction(SYSTEM_METADATA.clone()).build();
        let mut output = String::new();

        engine
            .redacted_display(&PanicMessage("secret panic".to_owned()), &mut output)
            .unwrap();

        assert!(!output.contains("secret panic"));
    }

    #[test]
    fn thread_exiting_carries_identity() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            ThreadExiting {
                name: Some(ThreadName::new("oxidizer-async-1")),
                id: ThreadId("ThreadId(7)".to_owned()),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.thread.exiting", Severity::Debug)
                .body("thread exiting")
                .dimension("thread.name", "oxidizer-async-1")
                .dimension("arty.thread.id", "ThreadId(7)")
        );
    }

    #[test]
    fn builtins_thread_mismatch_carries_backtrace() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            BuiltinsThreadMismatch {
                name: ThreadName::new("oxidizer-async-1"),
                id: ThreadId("ThreadId(7)".to_owned()),
                backtrace: BacktraceText("<backtrace>".to_owned()),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.builtins.thread_mismatch", Severity::Warn)
                .body("Builtins accessed from a different thread than it was created on")
                .dimension("thread.name", "oxidizer-async-1")
                .dimension("arty.thread.id", "ThreadId(7)")
                .dimension("backtrace", "<backtrace>")
        );
    }

    #[test]
    fn blocking_worker_pool_saturated_logs_and_counts() {
        let (sink, processor) = test_emitter(TEST_ID);

        emit!(
            sink,
            BlockingWorkerPoolSaturated {
                blocking_worker_pool_mode: BlockingWorkerPoolMode("isolated"),
                max_threads: SystemMetricCount(64),
            }
        );

        assert_eq!(
            processor.single_event(),
            ExpectedEvent::new("arty.rt.blocking_worker.pool_saturated", Severity::Warn)
                .body("blocking worker pool is saturated and cannot grow")
                .dimension("blocking_worker_pool.mode", "isolated")
                .dimension("blocking_worker_pool.max_threads", "64")
                .metric()
        );
    }
}
