// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for Tower interoperability.

use futures::executor::block_on;
use layered::tower::{tower_layer, Adapter};
use layered::{Execute, Service, Stack};
use std::task::Poll;
use tower::service_fn;
use tower_layer::Identity;
use tower_service::Service as TowerService;

#[test]
fn adapt_tower_ok() {
    let service = service_fn(|req: u32| async move { Ok::<_, ()>(req + 1) });
    let service = Adapter(service);

    let result = block_on(service.execute(0));

    assert_eq!(result, Ok(1));
}

#[test]
fn adapt_oxidizer_ok() {
    let service = Execute::new(|req: String| async move { Ok::<_, ()>(format!("Processed: {req}")) });
    let mut adapter = Adapter(service);

    let result = block_on(async move { adapter.call("request".to_string()).await });

    assert_eq!(result, Ok("Processed: request".to_string()));
}

#[test]
fn poll_ready_always_returns_ready_ok() {
    let service = Execute::new(|req: String| async move { Ok::<_, ()>(format!("Processed: {req}")) });
    let mut adapter = Adapter(service);

    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);

    let result = adapter.poll_ready(&mut cx);
    assert_eq!(result, Poll::Ready(Ok(())));
}

#[test]
fn poll_ready_consistent_behavior() {
    let service = Execute::new(|req: String| async move { Ok::<_, ()>(format!("Processed: {req}")) });
    let mut adapter = Adapter(service);

    let waker = futures::task::noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);

    // Multiple calls should return the same result
    for _ in 0..3 {
        let result = adapter.poll_ready(&mut cx);
        assert_eq!(result, Poll::Ready(Ok(())));
    }
}

#[test]
fn tower_layer_adapter() {
    let stack = (tower_layer(Identity::new()), Execute::new(|x: i32| async move { Ok::<_, ()>(x) }));
    let svc = stack.build();
    assert_eq!(block_on(svc.execute(42)), Ok(42));
}
