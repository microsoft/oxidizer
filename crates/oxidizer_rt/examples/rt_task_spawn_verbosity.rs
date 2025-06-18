// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases ways to spawn tasks with different levels of call verbosity.

use oxidizer_rt::{BasicThreadState, BasicThreadStateError, Placement, Runtime, TaskMeta};
use oxidizer_testing::log_to_console;

async fn async_main(cx: BasicThreadState) {
    explicit_spawn_custom_meta(&cx).await;
    explicit_spawn_default_meta(&cx).await;
    macro_spawn_custom_meta(&cx).await;
    macro_spawn_default_meta(&cx).await;
}

async fn explicit_spawn_custom_meta(cx: &BasicThreadState) {
    let task_meta = TaskMeta::builder()
        .name("my favorite async task")
        .placement(Placement::Any)
        .build();

    cx.scheduler()
        .spawn_with_meta(task_meta, async move |_| {
            println!("Hello from my favorite async task!");
        })
        .await;
}

async fn explicit_spawn_default_meta(cx: &BasicThreadState) {
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello from another async task!");
        })
        .await;
}

async fn macro_spawn_custom_meta(cx: &BasicThreadState) {
    let task_meta = TaskMeta::builder()
        .name("my favorite async task")
        .placement(Placement::Any)
        .build();

    cx.scheduler()
        .spawn_with_meta(task_meta, async move |_| {
            println!("Hello from one more async task!");
        })
        .await;
}

async fn macro_spawn_default_meta(cx: &BasicThreadState) {
    cx.scheduler()
        .spawn(async move |_| {
            println!("Hello from yet another async task!");
        })
        .await;
}

fn main() -> Result<(), BasicThreadStateError> {
    let _guard = log_to_console();

    let runtime = Runtime::new()?;

    // Runtime::spawn() is the minimally verbose non-macro version, only usable in `main()` due to
    // its reliance on blocking code (not permitted on Oxidizer Runtime owned threads).
    runtime.spawn(async move |_| {
        println!("Hello from some async task!");
    });

    runtime.run(async_main);
    Ok(())
}