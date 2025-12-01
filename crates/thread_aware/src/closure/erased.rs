// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{MemoryAffinity, PinnedAffinity};
use crate::{RelocateFnOnce, ThreadAware};

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
        C: RelocateFnOnce<T> + Clone + ThreadAware + 'static + Send + Sync,
    {
        Self {
            inner: Box::new(Wrapper { closure }),
        }
    }
}

impl<T> RelocateFnOnce<T> for ErasedClosureOnce<T> {
    fn call_once(self) -> T {
        self.inner.call_boxed_once()
    }
}

impl<T> ThreadAware for ErasedClosureOnce<T> {
    fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        self.inner.transfer_boxed(source, destination)
    }
}

impl<T> Clone for ErasedClosureOnce<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_boxed(),
        }
    }
}

trait Erased<T>: Sync + Send {
    fn call_boxed_once(self: Box<Self>) -> T;
    fn clone_boxed(&self) -> Box<dyn Erased<T>>;
    fn transfer_boxed(self: Box<Self>, source: MemoryAffinity, destination: PinnedAffinity) -> ErasedClosureOnce<T>;
}

struct Wrapper<C> {
    closure: C,
}

impl<T, C> Erased<T> for Wrapper<C>
where
    C: RelocateFnOnce<T> + Clone + ThreadAware + 'static + Send + Sync,
{
    fn call_boxed_once(self: Box<Self>) -> T {
        self.closure.call_once()
    }

    fn clone_boxed(&self) -> Box<dyn Erased<T>> {
        Box::new(Self {
            closure: self.closure.clone(),
        })
    }

    fn transfer_boxed(self: Box<Self>, source: MemoryAffinity, destination: PinnedAffinity) -> ErasedClosureOnce<T> {
        ErasedClosureOnce {
            inner: Box::new(Self {
                closure: self.closure.relocated(source, destination),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::closure::relocate_once;

    #[test]
    fn test_erased_closure_once_debug() {
        // Create an ErasedClosureOnce with a simple closure
        let closure = relocate_once(42, |x| x + 1);
        let erased = ErasedClosureOnce::new(closure);

        // Format using Debug trait - this covers line 14-15 (Debug::fmt)
        let debug_output = format!("{erased:?}");

        // Verify the output contains the expected debug information
        assert!(debug_output.contains("ErasedClosure"));
        assert!(debug_output.contains("return_type"));
        assert!(debug_output.contains("i32")); // The return type
    }

    #[test]
    fn test_erased_closure_once_debug_with_string() {
        // Create an ErasedClosureOnce that returns a String
        let closure = relocate_once("test", |s: &str| s.to_string());
        let erased = ErasedClosureOnce::new(closure);

        // Format using Debug trait
        let debug_output = format!("{erased:?}");

        // Verify the output contains String as the return type
        assert!(debug_output.contains("ErasedClosure"));
        assert!(debug_output.contains("String"));
    }
}
