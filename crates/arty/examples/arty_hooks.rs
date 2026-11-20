// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measure the duration of each async task poll.

use std::task::Poll;
use std::time::{Duration, Instant};

use arty::runtime::hook::{HookFn, TaskObserver, TaskSpawn};
use arty::task::{Context, JoinError};
use arty::time::Clock;

struct MeasurePoll {
    clock: Clock,
    started: Option<Instant>,
}

fn create_observer(clock: Clock) -> MeasurePoll {
    MeasurePoll { clock, started: None }
}

impl TaskObserver for MeasurePoll {
    fn poll_start(&mut self) {
        self.started = Some(self.clock.instant());
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {
        let started = self.started.take().expect("Arty calls end only after begin for the same task poll");
        let duration = self.clock.instant().saturating_duration_since(started);
        println!("task poll duration: {duration:?}");
    }
}

#[arty::main]
async fn main(cx: Context) -> Result<(), JoinError> {
    cx.control().on(HookFn::<TaskSpawn, _>::new(cx.clock().clone(), |event, clock| {
        event.observe_with(clock.clone(), create_observer);
    }));

    cx.scheduler()
        .spawn(|cx| async move {
            cx.clock().delay(Duration::from_secs(1)).await;
        })
        .await?;

    Ok(())
}
