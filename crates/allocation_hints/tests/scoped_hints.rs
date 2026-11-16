// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests scoped allocation hints through the public API.

use std::future::{poll_fn, ready};
use std::task::{Context, Poll, Waker};

use allocation_hints::heaps::{self, Heap, Kind, bump, thread_heap, thread_heap_request};
use allocation_hints::{active_hint, allocator_hints, with_hint, with_hint_async};

#[test]
fn nested_scopes_restore_the_previous_descriptor() {
    let outer = Heap::new();
    let inner = Heap::bump(bump::Options::new());

    with_hint(&outer, || {
        assert_eq!(active_hint().map(heaps::ActiveHint::id), Some(outer.id()));
        with_hint(&inner, || {
            assert_eq!(active_hint().map(heaps::ActiveHint::id), Some(inner.id()));
        });
        assert_eq!(active_hint().map(heaps::ActiveHint::id), Some(outer.id()));
    });
    assert_eq!(active_hint(), None);
}

#[test]
fn unused_prospective_heaps_require_no_backend() {
    let first = Heap::bump(bump::Options::new());
    let first_id = first.id();
    drop(first);
    let second = Heap::bump(bump::Options::new());
    assert_ne!(second.id(), first_id);
}

#[test]
fn thread_heap_captures_the_originating_thread() {
    let first = thread_heap();
    let second = thread_heap();

    assert_eq!(
        (first.kind(), first.id(), thread_heap_request().map(Kind::Thread)),
        (second.kind(), second.id(), Some(first.kind()))
    );
}

#[test]
fn allocator_hints_return_scoped_and_thread_requests_together() {
    let thread = thread_heap();
    let scoped = Heap::bump(bump::Options::new());

    with_hint(&scoped, || {
        let hints = allocator_hints();
        assert_eq!(
            (hints.active().map(heaps::ActiveHint::id), hints.thread_heap().map(Kind::Thread),),
            (Some(scoped.id()), Some(thread.kind()))
        );
    });
}

#[test]
fn unsupported_allocators_execute_operations_normally() {
    let heap = Heap::bump(bump::Options::new());
    let values = with_hint(&heap, || vec![1, 2, 3]);
    assert_eq!(values, [1, 2, 3]);
}

#[test]
fn async_hint_is_active_only_while_polling() {
    let heap = Heap::new();
    let future = poll_fn(|_| Poll::Ready(active_hint().map(heaps::ActiveHint::id)));
    let mut future = Box::pin(with_hint_async(&heap, future));
    let mut context = Context::from_waker(Waker::noop());

    assert_eq!(
        (future.as_mut().poll(&mut context), active_hint()),
        (Poll::Ready(Some(heap.id())), None)
    );
}

#[test]
fn async_hint_debug_includes_heap_and_future() {
    let heap = Heap::new();
    let future = with_hint_async(&heap, ready(42));

    assert_eq!(
        format!("{future:?}"),
        format!("WithHint {{ heap: {heap:?}, future: Ready(Some(42)) }}")
    );
}
