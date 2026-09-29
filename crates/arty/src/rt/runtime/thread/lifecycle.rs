// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::thread::{self, JoinHandle};

use observed::{Sink, emit};

use crate::rt::telemetry::events::{
    PanicMessage, SystemMetricCount, ThreadExiting, ThreadName, ThreadPanicked, ThreadSpawn, ThreadStarted,
};

/// Spawns a thread with a preferred name and stack size, reporting the thread's
/// lifecycle (spawn, start, panic, exit) through `sink`.
pub(in crate::rt::runtime) fn spawn<F>(sink: &Sink, name: &str, stack_size: Option<usize>, task: F) -> JoinHandle<()>
where
    F: FnOnce() + Send + 'static,
{
    let owner_thread = thread::current();
    emit!(
        sink,
        ThreadSpawn {
            name: ThreadName::new(name),
            stack_size_bytes: stack_size.map(SystemMetricCount::from),
            owner_name: owner_thread.name().map(ThreadName::new),
            owner_id: owner_thread.id().into(),
        }
    );

    let thread_sink = sink.clone();

    let mut builder = thread::Builder::new().name(name.to_string());
    if let Some(stack_size) = stack_size {
        builder = builder.stack_size(stack_size);
    }

    builder
        .spawn(move || {
            thread_fn(&thread_sink, task);
        })
        .expect("failed to spawn thread")
}

/// The function that runs on the spawned thread, executing the task
/// and reporting its lifecycle events.
fn thread_fn(sink: &Sink, task: impl FnOnce() + Send + 'static) {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let thread = thread::current();

    emit!(
        sink,
        ThreadStarted {
            name: thread.name().map(ThreadName::new),
            id: thread.id().into(),
        }
    );

    let result = catch_unwind(AssertUnwindSafe(task));

    if let Err(panic) = result {
        let panic_msg = if let Some(s) = panic.downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = panic.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_owned()
        };

        emit!(
            sink,
            ThreadPanicked {
                name: thread.name().map(ThreadName::new),
                id: thread.id().into(),
                message: PanicMessage(panic_msg),
            }
        );

        std::panic::resume_unwind(panic);
    }

    emit!(
        sink,
        ThreadExiting {
            name: thread.name().map(ThreadName::new),
            id: thread.id().into(),
        }
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use observed::Value;
    use observed_testing::{TEST_ID, test_emitter};

    use super::*;

    #[test]
    fn spawn_executes_task_successfully() {
        let result = Arc::new(Mutex::new(0));
        let result_clone = Arc::clone(&result);

        let handle = spawn(&Sink::noop(), "test-thread", None, move || {
            let mut value = result_clone.lock().unwrap();
            *value = 42;
        });

        handle.join().unwrap();

        let final_value = *result.lock().unwrap();
        assert_eq!(final_value, 42);
    }

    #[test]
    fn spawn_propagates_panic() {
        let handle = spawn(&Sink::noop(), "panicking-thread", None, || {
            panic!("task panicked");
        });

        let _ = handle.join().unwrap_err();
    }

    #[test]
    fn panicking_thread_emits_panic_event() {
        let (sink, processor) = test_emitter(TEST_ID);

        let handle = spawn(&sink, "panicking-thread", None, || {
            panic!("test test 123");
        });
        let _ = handle.join();

        let events = processor.events();
        let panic_event = events
            .iter()
            .find(|event| event.name() == "oxidizer.rt.thread.panic")
            .expect("a panicking thread emits oxidizer.rt.thread.panic");

        let attribute = |key: &str| -> Option<String> {
            panic_event
                .dimensions()
                .into_iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| match v {
                    Value::String(s) => Some(s.to_string()),
                    _ => None,
                })
        };

        assert_eq!(attribute("panic.message"), Some("test test 123".to_owned()));
        assert_eq!(attribute("thread.name"), Some("panicking-thread".to_owned()));
    }

    #[test]
    fn check_thread_naming() {
        const THREAD_NAME: &str = "named-thread";
        let handle = spawn(&Sink::noop(), THREAD_NAME, None, || {
            let current_thread = thread::current();
            assert_eq!(current_thread.name().unwrap(), THREAD_NAME);
        });

        handle.join().unwrap();
    }
}
