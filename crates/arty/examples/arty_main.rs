// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime entry-point macro with a local, non-Send result.

use std::rc::Rc;

use arty::rt::Builtins;

#[arty::rt::main]
async fn main(cx: Builtins) {
    let answer = cx
        .local_scheduler()
        .expect("the entry point runs on its associated worker")
        .spawn(async || Rc::new(42))
        .await;
    println!("{answer}");
}
