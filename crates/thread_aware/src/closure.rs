// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "TODO")]

pub mod erased;

pub use erased::ErasedClosureOnce;

use crate::ThreadAware;

/// A FnOnce-like, parameterless closure whose captured values all implement [`ThreadAware`]
///
/// Use [`relocate_once`] function to construct these.
pub trait RelocateFnOnce<T: ?Sized>: ThreadAware {
    /// Calls the closure, consuming it in the process.
    fn call_once(self) -> T;
}

/// A trait for callable types that can be called multiple times.
/// This trait is used to define closures that can be called multiple times, without consuming the closure.
pub trait RelocateFn<T>: ThreadAware {
    /// Calls the closure, returning the result.
    fn call(&self) -> T;
}

/// A trait for callable types that can be called mutably.
/// This trait is used to define closures that can be called mutably, allowing the closure to modify its internal state.
pub trait RelocateFnMut<T>: ThreadAware {
    /// Calls the closure mutably, returning the result.
    fn call_mut(&mut self) -> T;
}

/// A common implementation of [`RelocateFn`]
///
/// Construct this using the [`relocate`] function.
#[derive(Debug, Copy, Hash)]
pub struct Closure<T, D> {
    data: D,
    f: fn(&D) -> T,
}

impl<T, D> Clone for Closure<T, D>
where
    D: Clone,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D> RelocateFn<T> for Closure<T, D>
where
    D: ThreadAware,
{
    fn call(&self) -> T {
        (self.f)(&self.data)
    }
}

impl<T, D> RelocateFnMut<T> for Closure<T, D>
where
    D: ThreadAware,
{
    fn call_mut(&mut self) -> T {
        self.call()
    }
}

impl<T, D> RelocateFnOnce<T> for Closure<T, D>
where
    D: ThreadAware,
{
    fn call_once(self) -> T {
        self.call()
    }
}

impl<T, D> ThreadAware for Closure<T, D>
where
    D: ThreadAware,
{
    fn relocated(self, source: crate::MemoryAffinity, destination: crate::PinnedAffinity) -> Self {
        let data = self.data.relocated(source, destination);
        Self { data, f: self.f }
    }
}

/// A common implementation of [`RelocateFnOnce`]
///
/// Construct this using the [`relocate_once`] function.
#[derive(Debug, Copy, Hash)]
pub struct ClosureOnce<T, D> {
    data: D,
    f: fn(D) -> T,
}

impl<T, D> Clone for ClosureOnce<T, D>
where
    D: Clone,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D> RelocateFnOnce<T> for ClosureOnce<T, D>
where
    D: ThreadAware,
{
    fn call_once(self) -> T {
        (self.f)(self.data)
    }
}

impl<T, D> ThreadAware for ClosureOnce<T, D>
where
    D: ThreadAware,
{
    fn relocated(self, source: crate::MemoryAffinity, destination: crate::PinnedAffinity) -> Self {
        let data = self.data.relocated(source, destination);
        Self { data, f: self.f }
    }
}

/// A common implementation of [`RelocateFnMut`]]
///
/// Construct this using the [`relocate_mut`] function.
#[derive(Debug, Copy, Hash)]
pub struct ClosureMut<T, D> {
    data: D,
    f: fn(&mut D) -> T,
}

impl<T, D> Clone for ClosureMut<T, D>
where
    D: Clone,
{
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            f: self.f,
        }
    }
}

impl<T, D> RelocateFnMut<T> for ClosureMut<T, D>
where
    D: ThreadAware,
{
    fn call_mut(&mut self) -> T {
        (self.f)(&mut self.data)
    }
}

impl<T, D> RelocateFnOnce<T> for ClosureMut<T, D>
where
    D: ThreadAware,
{
    fn call_once(mut self) -> T {
        self.call_mut()
    }
}

impl<T, D> ThreadAware for ClosureMut<T, D>
where
    D: ThreadAware,
{
    fn relocated(self, source: crate::MemoryAffinity, destination: crate::PinnedAffinity) -> Self {
        let data = self.data.relocated(source, destination);
        Self { data, f: self.f }
    }
}

/// Construct a [`RelocateFn`] - a closure-like object where the captured data implement [`ThreadAware`].
///
/// Create a closure-like object by explicitly providing closed-over
/// value and a function pointer to operate on that value, essentially simulating a
/// parameterless closure that ensures that captured data implements [`ThreadAware`].
pub fn relocate<T, D>(data: D, f: fn(&D) -> T) -> Closure<T, D>
where
    D: ThreadAware,
{
    Closure { data, f }
}

/// Construct a [`RelocateFnMut`] - a closure-like object where the captured data implement [`ThreadAware`].
///
/// Create a closure-like object by explicitly providing closed-over
/// value and a function pointer to operate on that value, essentially simulating a
/// parameterless closure that ensures that captured data implements [`ThreadAware`].
pub fn relocate_mut<T, D>(data: D, f: fn(&mut D) -> T) -> ClosureMut<T, D>
where
    D: ThreadAware,
{
    ClosureMut { data, f }
}

/// Construct a [`RelocateFnOnce`] - a closure-like object where the captured data implement [`ThreadAware`].
///
/// Create a closure-like object by explicitly providing closed-over
/// value and a function pointer to operate on that value, essentially simulating a
/// parameterless closure that ensures that captured data implements [`ThreadAware`].
///
/// Usage:
/// ```rust
/// # use thread_aware::{PinnedAffinity, ThreadAware, MemoryAffinity, relocate_once, RelocateFnOnce};
/// struct Transferrable;
/// impl ThreadAware for Transferrable {
///     // ...
///     # fn relocated(self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
///     #    Self {}
///     # }
/// }
///
/// let closure = relocate_once(Transferrable, |transferrable| {
///     // do stuff with transferrable
/// });
///
/// closure.call_once();
///
/// let closure_with_multiple_captured = relocate_once((Transferrable, Transferrable), |(a, b)| {
///     // do stuff with a and b
/// });
///
/// closure_with_multiple_captured.call_once();
/// ```
///
/// This exists because Rust closures don't give us control over the types of captured values.
pub fn relocate_once<T, D>(data: D, f: fn(D) -> T) -> ClosureOnce<T, D>
where
    D: ThreadAware,
{
    ClosureOnce { data, f }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::create_manual_pinned_affinities;

    #[test]
    fn boxed_once() {
        let x = relocate_once(42, |x| x + 1);
        let y = Box::new(x);
        let _z = y.call_once();
    }

    #[test]
    fn more_stuff() {
        let x = Closure { data: 42, f: |x| x + 1 };

        let _y = x.call();
    }

    #[test]
    fn something() {
        fn takes_mut(mut x: impl RelocateFnMut<i32>) {
            let _y = x.call_mut();
        }

        let x = relocate(42, |x| x + 1);
        takes_mut(x);
    }

    #[allow(clippy::empty_structs_with_brackets, reason = " Testing non-clone behavior")]
    #[test]
    fn non_clone() {
        struct MyStruct {}

        let y = relocate((), |()| MyStruct {});
        let _z = y.call();
    }

    #[allow(clippy::redundant_clone, reason = "Testing clone behavior")]
    #[test]
    fn can_clone() {
        let x = Closure {
            data: String::from("Hello, world!"),
            f: |_| 43,
        };

        assert_eq!(x.call(), 43);
        let y = x.clone();
        assert_eq!(y.call(), 43);
    }

    // Tests for Closure<T, D>

    #[test]
    fn test_closure_clone() {
        // Test with Vec
        let closure = relocate(vec![1, 2, 3], std::vec::Vec::len);
        let cloned = closure.clone();
        assert_eq!(closure.call(), 3);
        assert_eq!(cloned.call(), 3);

        // Test with String
        let closure = Closure {
            data: String::from("test"),
            f: |s| s.len(),
        };

        let cloned = closure.clone();
        assert_eq!(closure.call(), 4);
        assert_eq!(cloned.call(), 4);
    }

    #[test]
    fn test_closure_thread_aware() {
        let affinities = create_manual_pinned_affinities(&[2, 2]);

        // Test with i32
        let closure = relocate(42_i32, |x| x + 1);
        let relocated = closure.relocated(affinities[0].into(), affinities[1]);
        assert_eq!(relocated.call(), 43);

        // Test with Vec
        let closure = relocate(vec![10, 20, 30], |v| v.iter().sum::<i32>());
        let relocated = closure.relocated(affinities[0].into(), affinities[2]);
        assert_eq!(relocated.call(), 60);

        // Test with same affinity (String)
        let closure = relocate(String::from("hello"), |s| s.to_uppercase());

        let relocated = closure.relocated(affinities[0].into(), affinities[0]);
        assert_eq!(relocated.call(), "HELLO");
    }

    #[test]
    fn test_closure_relocate_fn_mut() {
        // Test with i32 - multiple calls should give same result
        let mut closure = relocate(5_i32, |x| x * 2);
        assert_eq!(closure.call_mut(), 10);
        assert_eq!(closure.call_mut(), 10);

        // Test with Vec - multiple calls should give same result
        let mut closure = relocate(vec![1, 2, 3, 4], std::vec::Vec::len);

        assert_eq!(closure.call_mut(), 4);
        assert_eq!(closure.call_mut(), 4);
        assert_eq!(closure.call_mut(), 4);
    }

    // Tests for ClosureOnce<T, D>

    #[test]
    fn test_closure_once_clone() {
        // Test with i32
        let closure = relocate_once(100_i32, |x| x * 2);
        let cloned = closure;
        assert_eq!(closure.call_once(), 200);
        assert_eq!(cloned.call_once(), 200);

        // Test with complex data (tuple)
        let closure = ClosureOnce {
            data: (String::from("test"), vec![1, 2, 3]),
            f: |(s, v)| format!("{}: {}", s, v.len()),
        };

        let cloned = closure.clone();
        assert_eq!(closure.call_once(), "test: 3");
        assert_eq!(cloned.call_once(), "test: 3");
    }

    #[test]
    fn test_closure_once_thread_aware() {
        let affinities = create_manual_pinned_affinities(&[2, 3]);

        // Test with String
        let closure = relocate_once(String::from("world"), |s| format!("Hello, {s}!"));
        let relocated = closure.relocated(affinities[0].into(), affinities[1]);
        assert_eq!(relocated.call_once(), "Hello, world!");

        // Test with complex data (tuple of Vecs)
        let data = (vec![1, 2, 3], vec![4, 5, 6]);
        let closure = relocate_once(data, |(a, b)| a.len() + b.len());
        let relocated = closure.relocated(affinities[1].into(), affinities[3]);
        assert_eq!(relocated.call_once(), 6);

        // Test cross-NUMA transfer
        let closure = relocate_once(42_i32, |x| x + 100);
        let relocated = closure.relocated(affinities[0].into(), affinities[2]);
        assert_eq!(relocated.call_once(), 142);
    }

    // Tests for ClosureMut<T, D>

    #[test]
    fn test_closure_mut_clone() {
        // Test with i32 - clone creates independent copies
        let closure = relocate_mut(10_i32, |x| {
            *x += 5;
            *x
        });
        let cloned = closure;
        let mut c1 = closure;
        let mut c2 = cloned;
        assert_eq!(c1.call_mut(), 15);
        assert_eq!(c2.call_mut(), 15);

        // Test with Vec - independent copies continue to grow independently
        let closure = ClosureMut {
            data: vec![1, 2, 3],
            f: |v| {
                v.push(4);
                v.len()
            },
        };
        let cloned = closure.clone();
        let mut c1 = closure;
        let mut c2 = cloned;

        assert_eq!(c1.call_mut(), 4);
        assert_eq!(c1.call_mut(), 5); // Continues to grow
        assert_eq!(c2.call_mut(), 4); // Independent copy
    }

    #[test]
    fn test_closure_mut_thread_aware() {
        let affinities = create_manual_pinned_affinities(&[2, 3]);

        // Test with i32 - mutating state across relocations
        let closure = relocate_mut(0_i32, |x| {
            *x += 1;
            *x
        });
        let relocated = closure.relocated(affinities[0].into(), affinities[2]);
        let mut r = relocated;
        assert_eq!(r.call_mut(), 1);
        assert_eq!(r.call_mut(), 2);

        // Test with String - mutating string state
        let closure = relocate_mut(String::new(), |s| {
            s.push('x');
            s.len()
        });

        let relocated = closure.relocated(affinities[0].into(), affinities[2]);
        let mut r = relocated;
        assert_eq!(r.call_mut(), 1);
        assert_eq!(r.call_mut(), 2);
        assert_eq!(r.call_mut(), 3);
    }

    #[test]
    fn test_closure_mut_relocate_fn_mut() {
        let mut closure = relocate_mut(vec![1, 2], |v| {
            v.push(v.len() + 1);
            v.len()
        });

        assert_eq!(closure.call_mut(), 3);
        assert_eq!(closure.call_mut(), 4);
        assert_eq!(closure.call_mut(), 5);
    }

    #[test]
    fn test_closure_mut_relocate_fn_mut_independent_after_clone() {
        let closure = relocate_mut(0_i32, |x| {
            *x += 10;
            *x
        });

        let mut c1 = closure;
        let mut c2 = closure;

        assert_eq!(c1.call_mut(), 10);
        assert_eq!(c1.call_mut(), 20);
        assert_eq!(c2.call_mut(), 10);
        assert_eq!(c2.call_mut(), 20);
    }

    #[test]
    fn test_closure_mut_relocate_fn_once() {
        let closure = relocate_mut(String::from("test"), |s| {
            s.push('!');
            s.clone()
        });

        let result = closure.call_once();
        assert_eq!(result, "test!");
    }

    // Integration tests combining traits

    #[test]
    fn test_closure_all_traits_together() {
        let affinities = create_manual_pinned_affinities(&[2]);
        let closure = relocate(vec![1, 2, 3], std::vec::Vec::len);

        // Test Clone
        let cloned = closure;

        // Test ThreadAware
        let relocated = cloned.relocated(affinities[0].into(), affinities[1]);

        // Test RelocateFnMut
        let mut r = relocated;
        assert_eq!(r.call_mut(), 3);
    }

    #[test]
    fn test_closure_mut_all_traits_together() {
        let affinities = create_manual_pinned_affinities(&[2, 2]);
        let closure = relocate_mut(100_i32, |x| {
            *x += 1;
            *x
        });

        // Test Clone
        let cloned = closure;

        // Test ThreadAware across NUMA nodes
        let relocated = cloned.relocated(affinities[0].into(), affinities[3]);

        // Test RelocateFnMut
        let mut r = relocated;
        assert_eq!(r.call_mut(), 101);
        assert_eq!(r.call_mut(), 102);
    }

    #[test]
    fn test_closure_once_with_thread_aware_and_clone() {
        let affinities = create_manual_pinned_affinities(&[2]);
        let closure = relocate_once((1, 2, 3), |(a, b, c)| a + b + c);

        // Test Clone
        let cloned = closure;

        // Test ThreadAware
        let relocated = cloned.relocated(affinities[0].into(), affinities[1]);

        // Call once
        assert_eq!(relocated.call_once(), 6);

        // Original can still be used
        assert_eq!(closure.call_once(), 6);
    }
}
