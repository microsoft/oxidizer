// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task;

use mockall::mock;

use crate::{AbstractAsyncTaskExecutor, AsyncTask, CycleResult};

mock! {
    #[derive(Debug)]
    pub AsyncTask {}

    impl AsyncTask for AsyncTask {
        fn is_aborted(&self) -> bool;
        fn is_inert(&self) -> bool;
        fn clear(self: Pin<&mut Self>);
    }

    impl Future for AsyncTask {
        type Output = ();

        fn poll<'a>(self: Pin<&mut Self>, cx: &mut task::Context<'a>)
            -> task::Poll<<Self as Future>::Output>;
    }
}

mock! {
    #[derive(Debug)]
    pub AsyncTaskExecutor {}

    impl AbstractAsyncTaskExecutor for AsyncTaskExecutor {
        fn enqueue(&mut self, task: Pin<Box<dyn AsyncTask>>);
        fn execute_cycle(&mut self) -> CycleResult;
        fn begin_shutdown(&mut self);
    }
}