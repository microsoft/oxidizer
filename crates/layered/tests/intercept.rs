// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for Intercept middleware.

use futures::executor::block_on;
use layered::{Execute, Intercept, Layer, Service, Stack};
use static_assertions::assert_impl_all;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

#[test]
pub fn ensure_types() {
    assert_impl_all!(Intercept::<String, String, ()>: std::fmt::Debug, Clone, Send, Sync);
    assert_impl_all!(layered::InterceptLayer::<String, String>: std::fmt::Debug, Clone, Send, Sync);
}

#[test]
#[expect(clippy::similar_names, reason = "Test")]
fn input_modification_order() {
    let called = Arc::new(AtomicU16::default());
    let called_clone = Arc::clone(&called);

    let called2 = Arc::new(AtomicU16::default());
    let called2_clone = Arc::clone(&called2);

    let stack = (
        Intercept::layer()
            .modify_input(|input: String| format!("{input}1"))
            .modify_input(|input: String| format!("{input}2"))
            .on_input(move |_input| {
                called.fetch_add(1, Ordering::Relaxed);
            })
            .on_input(move |_input| {
                called2.fetch_add(1, Ordering::Relaxed);
            }),
        Execute::new(|input: String| async move { input }),
    );

    let service = stack.build();
    let response = block_on(service.execute("test".to_string()));
    assert_eq!(called_clone.load(Ordering::Relaxed), 1);
    assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
    assert_eq!(response, "test12");
}

#[test]
#[expect(clippy::similar_names, reason = "Test")]
fn out_modification_order() {
    let called = Arc::new(AtomicU16::default());
    let called_clone = Arc::clone(&called);

    let called2 = Arc::new(AtomicU16::default());
    let called2_clone = Arc::clone(&called2);

    let stack = (
        Intercept::layer()
            .modify_output(|output: String| format!("{output}1"))
            .modify_output(|output: String| format!("{output}2"))
            .on_output(move |_output| {
                called.fetch_add(1, Ordering::Relaxed);
            })
            .on_output(move |_output| {
                called2.fetch_add(1, Ordering::Relaxed);
            }),
        Execute::new(|input: String| async move { input }),
    );

    let service = stack.build();
    let response = block_on(service.execute("test".to_string()));
    assert_eq!(called_clone.load(Ordering::Relaxed), 1);
    assert_eq!(called2_clone.load(Ordering::Relaxed), 1);
    assert_eq!(response, "test12");
}

#[test]
fn debug_impls() {
    let layer = Intercept::<String, String, ()>::layer()
        .on_input(|_| {})
        .on_output(|_| {})
        .modify_input(|s| s)
        .modify_output(|s| s);
    assert!(format!("{layer:?}").contains("InterceptLayer"));
}
