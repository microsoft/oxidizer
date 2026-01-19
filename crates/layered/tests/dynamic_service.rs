// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for DynamicService.

use futures::executor::block_on;
use layered::{DynamicService, DynamicServiceExt, Execute, Service};
use static_assertions::assert_impl_all;
use std::sync::Mutex;

#[test]
fn assert_types() {
    assert_impl_all!(DynamicService<(), ()>: Send, Sync, Clone, std::fmt::Debug);

    // If non-clonable types are used, ensure the DynamicService is still cloneable
    assert_impl_all!(DynamicService<Mutex<()>, Mutex<()>>: Send, Sync, Clone, std::fmt::Debug);
}

#[test]
fn into_dynamic() {
    let dynamic_service: DynamicService<i32, i32> = Execute::new(|v| async move { v }).into_dynamic();

    assert_eq!(block_on(dynamic_service.execute(42)), 42);
}

#[test]
fn clone_and_debug() {
    let svc: DynamicService<i32, i32> = Execute::new(|v| async move { v }).into_dynamic();
    let cloned = svc.clone();
    assert_eq!(block_on(cloned.execute(1)), 1);
    assert_eq!(format!("{svc:?}"), "DynamicService");
}
