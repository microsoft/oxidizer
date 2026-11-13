// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stable opaque debug output.

use uniflight::Merger;

#[test]
fn gamma_survivor_debug_names_inner_without_exposing_keys() {
    let merger = Merger::<String, String, _>::new_per_process();
    assert_eq!(format!("{merger:?}"), "Merger { inner: DashMap<...> }");
}

#[test]
fn gamma_survivor_relocation_preserves_source_partition_for_existing_followers() {
    use std::future::Future as _;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};

    use performables::arc::PerNuma;
    use thread_aware::{ThreadAware as _, ThreadBuilder};

    let builder = ThreadBuilder::default();
    let source = builder.clone().with_numa_node(0).build(std::thread::current().id());
    let destination = builder.with_numa_node(1).build(std::thread::current().id());
    let merger = Merger::<u32, u32, PerNuma>::new();
    let mut moved = merger.clone();
    let mut returned = merger.clone();
    let released = Arc::new(AtomicBool::new(false));
    let follower_calls = Arc::new(AtomicUsize::new(0));
    let mut leader = pin!(merger.execute(&7, || std::future::poll_fn(|_| {
        if released.load(Ordering::Relaxed) {
            Poll::Ready(13)
        } else {
            Poll::Pending
        }
    })));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(leader.as_mut().poll(&mut cx).is_pending());
    moved.relocate(Some(&source), &destination);
    returned.relocate(Some(&destination), &source);
    let mut follower = pin!(returned.execute(&7, || async {
        follower_calls.fetch_add(1, Ordering::Relaxed);
        17
    }));
    assert!(follower.as_mut().poll(&mut cx).is_pending());
    assert_eq!(follower_calls.load(Ordering::Relaxed), 0);
    released.store(true, Ordering::Relaxed);
    assert!(matches!(leader.as_mut().poll(&mut cx), Poll::Ready(Ok(13))));
    assert!(matches!(follower.as_mut().poll(&mut cx), Poll::Ready(Ok(13))));
    assert_eq!(follower_calls.load(Ordering::Relaxed), 0);
}
