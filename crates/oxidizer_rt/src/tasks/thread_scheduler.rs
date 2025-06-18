// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

use crate::LocalTaskScheduler;

/// TODO: Temporary APIs until we fulls support local scheduler that is Send.
#[derive(Debug, Clone)]
pub struct ThreadScheduler {
    sender: async_channel::Sender<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

#[cfg_attr(test, mutants::skip)] // temporary code
impl ThreadScheduler {
    /// TODO: TEMPORARY, DO NOT USE!
    #[must_use]
    pub fn new(scheduler: &LocalTaskScheduler) -> Self {
        let (sender, receiver) = async_channel::unbounded();
        let clone = scheduler.clone();

        // Spawn a background job that schedules incoming tasks.
        scheduler.spawn(|| async move {
            while let Ok(future) = receiver.recv().await {
                // just schedule work, do not wait until it finishes
                let _handle = clone.spawn(|| async move {
                    future.await;
                });
            }
        });

        Self { sender }
    }

    /// TODO: TEMPORARY, DO NOT USE!
    pub fn spawn(
        &self,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), crate::Error> {
        let future = Box::pin(future);

        self.sender.try_send(future).map_err(|_e| {
            // Not really a programming error, but meh, this is temporary anyway.
            crate::Error::Programming(
                "unable to spawn a task in a thread because the async worker is shutting down "
                    .to_string(),
            )
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_send_sync() {
        static_assertions::assert_impl_all!(ThreadScheduler: Send, Sync);
    }
}