// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measure the duration of each async task poll.

use std::task::Poll;
use std::time::Duration;

use arty::runtime::hook::{HookFn, TaskObserver, TaskSpawn};
use arty::task::{Context, JoinError};
use tick::Stopwatch;

struct MeasurePoll(Stopwatch);

impl TaskObserver for MeasurePoll {
    fn poll_start(&mut self) {
        self.0.restart();
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {
        println!("task poll duration: {:?}", self.0.elapsed());
    }
}

#[arty::main]
async fn main(cx: Context) -> Result<(), JoinError> {
    cx.control().on(HookFn::<TaskSpawn, _>::new(cx.clock().clone(), |event, clock| {
        event.observe_with(clock.stopwatch(), MeasurePoll);
    }));

    cx.scheduler()
        .spawn(|cx| async move {
            cx.clock().delay(Duration::from_secs(1)).await;
        })
        .await?;

    Ok(())
}
