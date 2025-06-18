// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

use crate::{Domain, Transfer};

/// The `Inert` type can be used to wrap any foreign type that does not implement the `Transfer` trait.
///
/// It simply returns the wrapped value without performing any transfer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[repr(transparent)]
pub struct Inert<T>(pub T);

impl<T> Deref for Inert<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Inert<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> Transfer for Inert<T> {
    async fn transfer(self, _source: Domain, _destination: Domain) -> Self {
        self
    }
}

impl<T> Inert<T> {
    pub fn into_inner(self: Arc<Self>) -> Arc<T> {
        // SAFETY: `Inert` is a transparent wrapper around `T`,
        unsafe { std::mem::transmute(self) }
    }
}

#[cfg(test)]
mod tests {
    use super::{Inert, Transfer};
    use crate::create_domains;

    #[cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.
    #[oxidizer_rt::test]
    async fn test_inert(_context: oxidizer_rt::BasicThreadState) {
        let domains = create_domains(2);
        let inert = Inert(42);
        assert_eq!(*inert, 42);
        let transferred = inert.transfer(domains[0], domains[1]).await;
        assert_eq!(*transferred, 42);
    }
}