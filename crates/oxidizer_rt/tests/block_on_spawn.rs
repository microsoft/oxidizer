// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.

use oxidizer_rt::{BasicThreadState, Runtime};

#[test]
fn capture_by_value() {
    #[expect(
        clippy::useless_vec,
        reason = "using a vec to test with a type using pointers on the inside"
    )]
    let vec = vec![10, 11];
    let result = Runtime::<BasicThreadState>::new()
        .expect("Failed to create runtime")
        .block_on(async move |_ctx| vec.len() + 10);
    assert_eq!(result, 12);
}

#[test]
fn move_and_spawn() {
    #[expect(
        clippy::useless_vec,
        reason = "using a vec to test with a type using pointers on the inside"
    )]
    let vec = vec![10, 11];
    let result = Runtime::<BasicThreadState>::new()
        .expect("Failed to create runtime")
        .block_on(async move |ctx| {
            let res = vec.len() + 10;
            ctx.scheduler().spawn(async move |_ctx| vec.len() + 10);
            res
        });
    assert_eq!(result, 12);
}

#[test]
fn capture_reference() {
    let vec = vec![10, 11];
    let result = Runtime::<BasicThreadState>::new()
        .expect("Failed to create runtime")
        .block_on(|_ctx| {
            let vec_ref = &vec;
            async move { vec_ref.len() + 10 }
        });
    assert_eq!(vec.len() + 10, result);
}

#[test]
fn capture_mut_reference() {
    let mut vec = vec![10, 11];
    let result = Runtime::<BasicThreadState>::new()
        .expect("Failed to create runtime")
        .block_on(|_ctx| {
            let vec_ref = &mut vec;
            async move {
                vec_ref.push(15);
                vec_ref.len() + 10
            }
        });
    assert_eq!(vec.len() + 10, result);
    assert_eq!(vec.len(), 3);
}