// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`JoinHandle`] for awaiting spawned task results.

use std::fmt::Debug;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(feature = "custom")]
use futures_channel::oneshot;

/// A handle to a spawned task that can be awaited to retrieve its result.
///
/// This is returned by [`Spawner::spawn`](crate::Spawner::spawn) and implements
/// [`Future`] to allow awaiting the task's completion.
///
/// # Panics
///
/// Awaiting a `JoinHandle` will panic if the spawned task panicked.
pub struct JoinHandle<T>(pub(crate) JoinHandleInner<T>);

pub(crate) enum JoinHandleInner<T> {
    #[cfg(feature = "tokio")]
    Tokio(::tokio::task::JoinHandle<T>),
    #[cfg(feature = "custom")]
    Custom(oneshot::Receiver<T>),
    #[expect(
        dead_code,
        reason = "Unconstructable variant so the type compiles when no runtime features are enabled."
    )]
    #[cfg(not(any(feature = "custom", feature = "tokio")))]
    None(std::marker::PhantomData<fn() -> T>),
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    #[cfg_attr(
        not(any(feature = "custom", feature = "tokio")),
        expect(unused_variables, reason = "cx is unused depending on features")
    )]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().0 {
            #[cfg(feature = "tokio")]
            JoinHandleInner::Tokio(jh) => Pin::new(jh).poll(cx).map(|res| res.expect("spawned task panicked")),
            #[cfg(feature = "custom")]
            JoinHandleInner::Custom(rx) => Pin::new(rx).poll(cx).map(|res| res.expect("spawned task panicked")),
            #[cfg(not(any(feature = "custom", feature = "tokio")))]
            JoinHandleInner::None(_) => unreachable!("JoinHandleInner::None cannot be constructed"),
        }
    }
}

impl<T> Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinHandle").finish_non_exhaustive()
    }
}
