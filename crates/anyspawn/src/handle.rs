// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`JoinHandle`] for awaiting spawned task results.

use std::fmt::Debug;
use std::pin::Pin;
#[cfg(feature = "arty")]
use std::sync::Mutex;
use std::task::{Context, Poll};

use performables::sync::channel::OneshotReceiver;

#[cfg(feature = "arty")]
type ArtyJoinFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A handle to a spawned task that can be awaited to retrieve its result.
///
/// This is returned by [`Spawner::spawn`](crate::Spawner::spawn) and implements
/// [`Future`] to allow awaiting the task's completion.
///
/// # Panics
///
/// Awaiting a `JoinHandle` will panic if the spawned task panicked or its
/// runtime stopped before delivering the result.
pub struct JoinHandle<T>(pub(crate) JoinHandleInner<T>);

pub(crate) enum JoinHandleInner<T> {
    #[cfg(feature = "arty")]
    Arty(Mutex<ArtyJoinFuture<T>>),
    #[cfg(feature = "tokio")]
    Tokio(::tokio::task::JoinHandle<T>),
    Custom(OneshotReceiver<T>),
}

impl<T> JoinHandle<T> {
    #[cfg(feature = "arty")]
    pub(crate) fn from_arty(handle: ::arty::task::JoinHandle<T>) -> Self
    where
        T: Send + 'static,
    {
        let future = async move { unwrap_arty_result(handle.await) };
        Self(JoinHandleInner::Arty(Mutex::new(Box::pin(future))))
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().0 {
            #[cfg(feature = "arty")]
            JoinHandleInner::Arty(future) => future
                .get_mut()
                .expect("Arty join future mutex is never locked and therefore cannot be poisoned")
                .as_mut()
                .poll(cx),
            #[cfg(feature = "tokio")]
            JoinHandleInner::Tokio(jh) => Pin::new(jh).poll(cx).map(unwrap_tokio_result),
            JoinHandleInner::Custom(rx) => Pin::new(rx)
                .poll(cx)
                .map(|res| res.expect("spawned task did not produce a result because its channel closed")),
        }
    }
}

#[cfg(feature = "arty")]
#[expect(clippy::panic, reason = "JoinHandle documents runtime join failures as panics")]
fn unwrap_arty_result<T>(result: Result<T, arty::task::JoinError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("spawned task did not complete: {error}"),
    }
}

#[cfg(feature = "tokio")]
#[expect(clippy::panic, reason = "JoinHandle documents runtime join failures as panics")]
fn unwrap_tokio_result<T>(result: Result<T, tokio::task::JoinError>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("spawned task did not complete: {error}"),
    }
}

impl<T> Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}
