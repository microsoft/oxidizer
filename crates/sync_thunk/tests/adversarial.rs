// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg(not(loom))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Tests for adversarial usage patterns — `mem::forget`, cancellation, etc.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::time::Duration;

use sync_thunk::{Thunker, thunk};

/// Creates a no-op waker for manual polling.
fn noop_waker() -> Waker {
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    // SAFETY: The vtable functions are valid no-ops.
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

struct Service {
    thunker: Thunker,
}

impl Service {
    #[thunk(from = me.thunker)]
    async fn blocking_work(me: Arc<Self>, flag: Arc<AtomicBool>) -> u64 {
        // Signal that the worker started executing.
        flag.store(true, Ordering::Release);
        std::thread::sleep(Duration::from_millis(10));
        let _ = me;
        42
    }

    #[thunk(from = me.thunker)]
    async fn panicking_work(me: Arc<Self>, msg: String) -> u64 {
        let _ = me;
        #[expect(clippy::panic, reason = "deliberately panicking to test payload propagation")]
        {
            panic!("{msg}")
        }
    }
}

/// Service used to test deadlock-on-provider-panic. The `Thunker` lives
/// behind an accessor that panics; the wrapper future must NOT hang the
/// caller in `StackState::Drop` when the provider expression panics
/// before the work item is dispatched.
struct PanickingProvider {
    real: Thunker,
    sabotage: AtomicBool,
}

impl PanickingProvider {
    fn thunker(&self) -> &Thunker {
        assert!(!self.sabotage.load(Ordering::Acquire), "provider accessor deliberately panicked");
        &self.real
    }

    #[thunk(from = me.thunker())]
    async fn work(me: Arc<Self>) -> u64 {
        let _ = me;
        7
    }
}

/// Service used to verify `&'static T` reference parameters are accepted.
/// `'static` references cannot be invalidated by `mem::forget(future)`
/// because the referent lives for the entire program — there is no UAF
/// hazard equivalent to non-`'static` borrows.
struct StaticRefSvc {
    thunker: Thunker,
}

static SHARED_CONST: &str = "hello-from-static-storage";

impl StaticRefSvc {
    #[thunk(from = me.thunker)]
    async fn read_static(me: Arc<Self>, data: &'static str) -> usize {
        let _ = me;
        data.len()
    }
}

/// Calling `mem::forget` on a thunked future after it has been polled must
/// not cause UB.
///
/// With the (post-U1-fix) `spawn_blocking`-style soundness model, the future
/// owns its parameters outright: when it is forgotten, the parameters live on
/// inside the leaked allocation, the worker thread completes against still-
/// valid memory, and the result is a (sound) memory leak.
#[tokio::test]
async fn mem_forget_on_thunked_future_does_not_cause_ub() {
    let service = Arc::new(Service {
        thunker: Thunker::builder()
            .max_thread_count(2)
            .cool_down_interval(Duration::from_secs(1))
            .build(),
    });

    let started = Arc::new(AtomicBool::new(false));

    // Box::pin so we can forget the Box (and thus the StackState inside).
    let mut future = Box::pin(Service::blocking_work(Arc::clone(&service), Arc::clone(&started)));

    let waker = noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    let _ = std::future::Future::poll(future.as_mut(), &mut cx);

    // Forget the boxed future. Its destructor (StackState::Drop) never runs.
    // The allocation is leaked, but the worker still has a valid pointer
    // because the parameters were *moved* into the leaked future and live
    // until the worker drops them.
    std::mem::forget(future);

    std::thread::sleep(Duration::from_millis(100));

    assert!(started.load(Ordering::Acquire), "worker should have executed");
}

/// Panics inside a thunked body must be re-raised on the awaiter's task with
/// the original payload preserved (downcastable to the same type as a
/// synchronous panic), matching `std::panic::resume_unwind` behaviour.
#[tokio::test]
async fn panic_payload_is_preserved_across_thunk_boundary() {
    let service = Arc::new(Service {
        thunker: Thunker::builder().max_thread_count(1).build(),
    });

    let result = tokio::task::spawn(async move { Service::panicking_work(Arc::clone(&service), String::from("boom-123")).await }).await;

    match result {
        Ok(_) => panic!("expected the task to fail with a propagated panic"),
        Err(join_err) => {
            assert!(join_err.is_panic(), "task did not panic");
            let payload = join_err.into_panic();
            // The panic was raised via `panic!("{msg}")`, which produces a
            // `String` payload. Verify the exact value made it across.
            let s = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&'static str>().copied())
                .expect("expected a String/&str panic payload");
            assert_eq!(s, "boom-123");
        }
    }
}

/// Regression guard for L1: if the provider expression panics during
/// evaluation (before the work item is enqueued), the wrapper future
/// must NOT deadlock the caller in `StackState::Drop`. The
/// `__AbandonOnPanic` guard installed before provider evaluation
/// releases the spin loop so the caller's stack can unwind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_expression_panic_does_not_deadlock_caller() {
    let real = Thunker::builder().max_thread_count(1).build();
    let svc = Arc::new(PanickingProvider {
        real,
        sabotage: AtomicBool::new(true),
    });

    let svc_clone = Arc::clone(&svc);
    let join = tokio::task::spawn(async move { PanickingProvider::work(svc_clone).await });

    // Cross-thread sentinel: if the bug is present, the spawned task
    // hangs in `StackState::Drop` and we never receive on the channel.
    // The OS-thread timeout lets the test fail with a clear message
    // instead of stalling forever.
    let (tx, rx) = std::sync::mpsc::channel::<std::thread::Result<()>>();
    let join_for_thread = tokio::spawn(async move {
        let outcome = join.await;
        let _ = tx.send(outcome.map(|_| ()).map_err(|e| e.into_panic()));
    });

    let result = rx.recv_timeout(Duration::from_secs(5));
    if result.is_err() {
        // Cancel the spawned task so we don't leak it past the test.
        join_for_thread.abort();
        panic!("provider-panic deadlock regressed: caller hung in StackState::Drop");
    }
    let outcome = result.unwrap();
    let payload = outcome.expect_err("expected the task to panic from the provider expression");
    // Panic payload should be the message from the accessor.
    let s = payload
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .expect("expected a string panic payload");
    assert!(s.contains("provider accessor deliberately panicked"));
}

/// Regression test for E3: `&'static T` parameters are accepted and
/// usable. Non-`'static` borrows remain rejected (see `compile_fail`
/// doctests in `crates/sync_thunk/src/macros.rs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_reference_parameter_is_accepted() {
    let svc = Arc::new(StaticRefSvc {
        thunker: Thunker::builder().max_thread_count(1).build(),
    });
    let n = StaticRefSvc::read_static(svc, SHARED_CONST).await;
    assert_eq!(n, SHARED_CONST.len());
}

// E2 regression: arg types that mention `Self` in multi-segment paths
// (e.g. `Self::Output`, `<Self as Trait>::Bar`) and complex shapes like
// `Vec<Arc<Self>>` must compile. The macro lifts any `Self`-containing
// arg type into a fresh generic parameter on the local `__RawTask`
// struct, so the impl's `Self` does not have to leak through the nested
// item barrier.
trait HasOutput {
    type Output;
}

struct SelfTypesSvc {
    thunker: Thunker,
}

impl HasOutput for SelfTypesSvc {
    type Output = u64;
}

impl SelfTypesSvc {
    #[thunk(from = me.thunker)]
    async fn assoc_typed(me: Arc<Self>, value: <Self as HasOutput>::Output) -> u64 {
        let _ = me;
        value * 2
    }

    #[thunk(from = me.thunker)]
    async fn vec_of_self(me: Arc<Self>, others: Vec<Arc<Self>>) -> usize {
        let _ = me;
        others.len()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn self_in_param_types_works() {
    let svc = Arc::new(SelfTypesSvc {
        thunker: Thunker::builder().max_thread_count(1).build(),
    });
    let r = SelfTypesSvc::assoc_typed(Arc::clone(&svc), 21).await;
    assert_eq!(r, 42);

    let buddies = vec![Arc::clone(&svc), Arc::clone(&svc), Arc::clone(&svc)];
    let n = SelfTypesSvc::vec_of_self(svc, buddies).await;
    assert_eq!(n, 3);
}
