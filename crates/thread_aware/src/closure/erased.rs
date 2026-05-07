// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::ThreadAware;
use crate::affinity::Affinity;
use crate::closure::ThreadAwareFnOnce;

/// A closure with erased bounds.
pub struct ErasedClosureOnce<T: ?Sized> {
    inner: Box<dyn Erased<T>>,
}

//TODO Refactor and call debug on the inner closure
impl<T: ?Sized> std::fmt::Debug for ErasedClosureOnce<T> {
    #[cfg_attr(test, mutants::skip)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErasedClosure")
            .field("return_type", &std::any::type_name::<T>())
            .finish_non_exhaustive()
    }
}

impl<T> ErasedClosureOnce<T> {
    /// Creates a new closure with erased bounds.
    pub fn new<C>(closure: C) -> Self
    where
        C: ThreadAwareFnOnce<T> + Clone + ThreadAware + 'static + Send + Sync,
    {
        Self {
            inner: Box::new(Wrapper { closure }),
        }
    }
}

impl<T> ThreadAwareFnOnce<T> for ErasedClosureOnce<T> {
    fn call_once(self) -> T {
        self.inner.call_boxed_once()
    }
}

impl<T> ThreadAware for ErasedClosureOnce<T> {
    fn relocate(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.inner.transfer_boxed_mut(source, destination);
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
    fn transfer_boxed_mut(&mut self, source: Option<Affinity>, destination: Affinity);
}

struct Wrapper<C> {
    closure: C,
}

impl<T, C> Erased<T> for Wrapper<C>
where
    C: ThreadAwareFnOnce<T> + Clone + ThreadAware + 'static + Send + Sync,
{
    fn call_boxed_once(self: Box<Self>) -> T {
        self.closure.call_once()
    }

    fn clone_boxed(&self) -> Box<dyn Erased<T>> {
        Box::new(Self {
            closure: self.closure.clone(),
        })
    }

    fn transfer_boxed_mut(&mut self, source: Option<Affinity>, destination: Affinity) {
        self.closure.relocate(source, destination);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::closure::closure_once;

    #[test]
    fn test_erased_closure_once_debug() {
        // Create an ErasedClosureOnce with a simple closure
        let closure = closure_once(42, |x| x + 1);
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
        let closure = closure_once("test", |s: &str| s.to_string());
        let erased = ErasedClosureOnce::new(closure);

        // Format using Debug trait
        let debug_output = format!("{erased:?}");

        // Verify the output contains String as the return type
        assert!(debug_output.contains("ErasedClosure"));
        assert!(debug_output.contains("String"));
    }

    /// A type whose `relocate` visibly mutates state.
    #[derive(Clone)]
    struct Tracker(bool);

    impl crate::ThreadAware for Tracker {
        fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
            self.0 = true;
        }
    }

    #[test]
    fn erased_closure_once_relocate_forwards_to_inner() {
        use crate::affinity::pinned_affinities;
        use crate::closure::ThreadAwareFnOnce;

        let affinities = pinned_affinities(&[2]);
        let src = Some(affinities[0]);
        let dst = affinities[1];

        let c = closure_once(Tracker(false), |t: Tracker| t.0);
        let mut erased = ErasedClosureOnce::new(c);
        erased.relocate(src, dst);

        let result = erased.call_once();
        assert!(result, "ErasedClosureOnce must forward relocate to inner closure");
    }
}
