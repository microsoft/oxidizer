// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Register and print a panic hook from an async entry point.

use arty::runtime::hook::OnPanic;
use arty::task::{Context, JoinError};

#[arty::main]
#[expect(clippy::panic, reason = "this example demonstrates runtime panic hook behavior")]
async fn main(cx: Context) -> Result<(), JoinError> {
    cx.control().on::<OnPanic, _>((), |event, ()| {
        println!("{event:?}");
        event.ignore();
    });

    let panic = cx.scheduler().spawn(async |_| panic!("contained panic")).await.unwrap_err();
    assert!(panic.is_panic());
    assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
    Ok(())
}
