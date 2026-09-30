// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime entry-point macro with a local, non-Send result.

use std::rc::Rc;

use arty::runtime::Builtins;

#[arty::main]
async fn main(cx: Builtins) {
    let answer = cx
        .local_scheduler()
        .expect("the entry point runs on its associated worker")
        .spawn(async || Rc::new(42))
        .await
        .expect("the local task completes before the entry point returns");
    println!("{answer}");
}
