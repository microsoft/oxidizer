// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

use crate::transfer::Domain;
use crate::{Transfer, TransferFnOnce};

pub struct ErasedClosureOnce<T> {
    inner: Box<dyn Erased<T>>,
}

//TODO Refactor and call debug on the inner closure
impl<T> std::fmt::Debug for ErasedClosureOnce<T> {
    #[mutants::skip]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErasedClosure")
            .field("return_type", &std::any::type_name::<T>())
            .finish_non_exhaustive()
    }
}

impl<T> ErasedClosureOnce<T> {
    pub fn new<C>(closure: C) -> Self
    where
        C: TransferFnOnce<T> + Clone + Transfer + 'static,
    {
        Self {
            inner: Box::new(Wrapper { closure }),
        }
    }
}

impl<T> TransferFnOnce<T> for ErasedClosureOnce<T> {
    fn call_once(self) -> T {
        self.inner.call_boxed_once()
    }
}

impl<T> Transfer for ErasedClosureOnce<T> {
    async fn transfer(self, source: Domain, destination: Domain) -> Self {
        self.inner.transfer_boxed(source, destination).await
    }
}

impl<T> Clone for ErasedClosureOnce<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_boxed(),
        }
    }
}

trait Erased<T> {
    fn call_boxed_once(self: Box<Self>) -> T;
    fn clone_boxed(&self) -> Box<dyn Erased<T>>;
    fn transfer_boxed(
        self: Box<Self>,
        source: Domain,
        destination: Domain,
    ) -> Pin<Box<dyn Future<Output = ErasedClosureOnce<T>>>>;
}

struct Wrapper<C> {
    closure: C,
}

impl<T, C> Erased<T> for Wrapper<C>
where
    C: TransferFnOnce<T> + Clone + Transfer + 'static,
{
    fn call_boxed_once(self: Box<Self>) -> T {
        self.closure.call_once()
    }

    fn clone_boxed(&self) -> Box<dyn Erased<T>> {
        Box::new(Self {
            closure: self.closure.clone(),
        })
    }

    fn transfer_boxed(
        self: Box<Self>,
        source: Domain,
        destination: Domain,
    ) -> Pin<Box<dyn Future<Output = ErasedClosureOnce<T>>>> {
        Box::pin(async move {
            ErasedClosureOnce {
                inner: Box::new(Self {
                    closure: self.closure.transfer(source, destination).await,
                }),
            }
        })
    }
}