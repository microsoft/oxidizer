// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Special purpose example to explore effects of spawning and completing millions of tasks.

use std::cell::Cell;
use std::rc::Rc;
use std::{env, iter};

use arty_executor::{CycleOutcome, Executor, TaskOptions};
use nm::Report;

// Two layers multiplied together is 10 million.
const FIRST_LAYER_TASK_COUNT: usize = 1_000;
const SECOND_LAYER_TASK_COUNT: usize = 10_000;
const TEST_FIRST_LAYER_TASK_COUNT: usize = 10;
const TEST_SECOND_LAYER_TASK_COUNT: usize = 100;

fn main() {
    let (first_layer_task_count, second_layer_task_count) = if env::var_os("ANVIL_EXAMPLE").is_some() {
        (TEST_FIRST_LAYER_TASK_COUNT, TEST_SECOND_LAYER_TASK_COUNT)
    } else {
        (FIRST_LAYER_TASK_COUNT, SECOND_LAYER_TASK_COUNT)
    };

    let executor = Executor::builder().build();
    let tasks = executor.tasks();

    // When our root task is done, it will set this signal to indicate that the app can exit.
    let shutdown_signal = Rc::new(Cell::new(false));

    tasks.add(
        {
            let tasks = tasks.clone();
            let shutdown_signal = Rc::clone(&shutdown_signal);

            // The root task, which ultimately shuts down the executor.
            async move {
                let join_handles = iter::repeat_with(|| {
                    tasks.add(
                        {
                            let tasks = tasks.clone();

                            // The first layer of spawned tasks.
                            async move {
                                let join_handles = iter::repeat_with(|| {
                                    tasks.add(
                                        async move {
                                            // This task does nothing, it only exists to complete.
                                        },
                                        TaskOptions::default(),
                                    )
                                })
                                .take(second_layer_task_count)
                                .collect::<Vec<_>>();

                                for join_handle in join_handles {
                                    join_handle.await.expect("the executor remains active until all work completes");
                                }
                            }
                        },
                        TaskOptions::default(),
                    )
                })
                .take(first_layer_task_count)
                .collect::<Vec<_>>();

                for join_handle in join_handles {
                    join_handle.await.expect("the executor remains active until all work completes");
                }

                shutdown_signal.set(true);
            }
        },
        TaskOptions::default(),
    );

    let mut shutdown_started = false;

    while executor.execute_cycle() != CycleOutcome::Shutdown {
        if !shutdown_started && shutdown_signal.get() {
            shutdown_started = true;

            executor.begin_shutdown();
        }
    }

    // Dump metrics at the end.
    println!("{}", Report::collect());
}
