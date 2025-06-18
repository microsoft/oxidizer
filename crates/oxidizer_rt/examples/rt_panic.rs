// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use oxidizer_rt::{BasicThreadState, main};

// Validate that when the runtime encounters a panic, the user sees it in their terminal,
// as opposed to it being hidden behind some other error message for whatever reason.
#[main]
async fn main(cx: BasicThreadState) {
    cx.local_scheduler()
        .spawn(async move || {
            panic!("this is a panic and we expect it to be visible in the terminal")
        })
        .await;
}