// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measure the duration of each async task poll.

use std::time::{Duration, Instant};

use arty::runtime::hook::{HookFn, Spawn, TaskPoll};
use arty::task::{Context, JoinError};
use arty::time::Clock;
use thread_aware::ThreadAware;

#[derive(ThreadAware)]
struct MeasurePoll {
    clock: Clock,
    #[thread_aware(skip)]
    started: Option<Instant>,
}

impl TaskPoll for MeasurePoll {
    fn begin(&mut self) {
        self.started = Some(self.clock.instant());
    }

    fn end(&mut self) {
        let started = self.started.take().expect("Arty calls end only after begin for the same task poll");
        let duration = self.clock.instant().saturating_duration_since(started);
        println!("task poll duration: {duration:?}");
    }
}

#[arty::main]
async fn main(cx: Context) -> Result<(), JoinError> {
    cx.control().on(HookFn::<Spawn, _>::new(cx.clock().clone(), |event, clock| {
        event.register(MeasurePoll {
            clock: clock.clone(),
            started: None,
        });
    }));

    cx.scheduler()
        .spawn(|cx| async move {
            cx.clock().delay(Duration::from_secs(1)).await;
        })
        .await?;

    Ok(())
}
