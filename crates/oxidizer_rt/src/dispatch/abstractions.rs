// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::{Instantiation, Placement, RemoteJoinHandle, RuntimeThreadState, SpawnInstance};

/// This is an internal trait that facilitates the implementation of the public APIs exposed by
/// the various `*TaskContext` types and the `Runtime` type. See API docs on those types to
/// understand what this trait contains and why.
///
/// The type implementing this trait is cloned for each task, which is why this trait is `Clone`
/// and requires the `'static` lifetime - the instances are all clients of a single shared runtime.
pub trait Dispatch: DispatchStop + Clone + 'static
where
    Self: Sized,
{
    type ThreadState: RuntimeThreadState;

    fn spawn<FF, F, R>(&self, placement: Placement, future_factory: FF) -> RemoteJoinHandle<R>
    where
        FF: FnOnce(Self::ThreadState) -> F + Send + 'static,
        F: Future<Output = R> + 'static,
        R: Send + 'static;

    fn spawn_multiple<FF, F, R>(
        &self,
        placement: Placement,
        instantiation: Instantiation,
        future_factory: FF,
    ) -> Box<[RemoteJoinHandle<R>]>
    where
        FF: MultiInstanceFutureFactory<Self::ThreadState, F, R>,
        F: Future<Output = R> + 'static,
        R: Send + 'static;
}

/// A separate dyn compatible trait for Stop to allow `RuntimeOperations` without type parameters. This inherits
/// from Debug to allow Box<dyn DispatchStop> values to preserve their Debug information.
pub trait DispatchStop: Debug {
    fn stop(&self);
    fn wait(&self);
}

/// `MultiInstanceFutureFactory` is a helper supertrait for `FnOnce` that used in [`Dispatch`] trait.
///
/// It is required to mitigate [mockall issue](https://github.com/asomers/mockall/issues/623).
pub trait MultiInstanceFutureFactory<C, F, R>:
    FnOnce(C, SpawnInstance) -> F + Clone + Send + 'static
where
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
}

impl<FF, C, F, R> MultiInstanceFutureFactory<C, F, R> for FF
where
    FF: FnOnce(C, SpawnInstance) -> F + Clone + Send + 'static,
    F: Future<Output = R> + 'static,
    R: Send + 'static,
{
}