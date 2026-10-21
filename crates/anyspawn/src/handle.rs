// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`JoinHandle`] for awaiting spawned task results.

use std::fmt::Debug;
use std::pin::Pin;
use std::task::{Context, Poll};

use performables::sync::channel::OneshotReceiver;
use pin_project::pin_project;

/// A handle to a spawned task that can be awaited to retrieve its result.
///
/// This is returned by [`Spawner::spawn`](crate::Spawner::spawn) and implements
/// [`Future`] to allow awaiting the task's completion.
///
/// # Panics
///
/// Awaiting a `JoinHandle` will panic if the spawned task panicked or its
/// runtime stopped before delivering the result.
#[pin_project]
pub struct JoinHandle<T: Send + 'static>(#[pin] pub(crate) JoinHandleInner<T>);

#[pin_project(project = JoinHandleInnerProj)]
pub(crate) enum JoinHandleInner<T: Send + 'static> {
    #[cfg(feature = "arty")]
    Arty(#[pin] ::arty::task::JoinHandle<T>),
    #[cfg(feature = "tokio")]
    Tokio(#[pin] ::tokio::task::JoinHandle<T>),
    Custom(#[pin] OneshotReceiver<T>),
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().0.project() {
            #[cfg(feature = "arty")]
            JoinHandleInnerProj::Arty(jh) => jh.poll(cx).map(unwrap_arty_result),
            #[cfg(feature = "tokio")]
            JoinHandleInnerProj::Tokio(jh) => jh.poll(cx).map(unwrap_tokio_result),
            JoinHandleInnerProj::Custom(rx) => rx
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

impl<T: Send + 'static> Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}
