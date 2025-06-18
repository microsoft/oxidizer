// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::use_debug,
    reason = "using debug formatting for example purposes"
)]

use oxidizer_rt::{BasicThreadState, Placement, main};

#[main]
async fn main(cx: BasicThreadState) {
    // Same thread spawn using the join handle
    let handle = cx.scheduler().spawn(async move |_| {
        println!("Hello from thread {:?}", std::thread::current().id());
    });

    cx.scheduler()
        .spawn_with_meta(
            Placement::SameThreadAs(handle.placement().unwrap()),
            async move |_| {
                println!("Hello from thread {:?} again", std::thread::current().id());
            },
        )
        .await;

    // Same thread spawn using the context placement
    let placement = cx
        .scheduler()
        .spawn(async move |cx| {
            println!(
                "Hello from thread {:?}, returning my placement",
                std::thread::current().id()
            );
            cx.runtime_ops().placement().unwrap()
        })
        .await;

    cx.scheduler()
        .spawn_with_meta(Placement::SameThreadAs(placement), async move |_| {
            println!("Hello from thread {:?} again", std::thread::current().id());
        })
        .await;
}