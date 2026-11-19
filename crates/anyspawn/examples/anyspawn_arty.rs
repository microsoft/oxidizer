// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Spawning tasks with an Arty runtime.

use anyspawn::{JoinError, Spawner};
use arty::runtime::Runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Runtime::new()?;
    runtime.scheduler().block_on(async |cx| {
        let spawner = Spawner::new_arty(cx.scheduler().clone());

        spawner
            .spawn(async {
                println!("Background task completed!");
            })
            .await?;

        let value = spawner.spawn(async { 1 + 1 }).await?;
        println!("Got result: {value}");
        Ok::<(), JoinError>(())
    })??;
    runtime.stop()?;
    Ok(())
}
